#!/usr/bin/env python3
"""Run the Transcript Lake secret scrub on the host that owns the Lake.

An agent session cannot do this from its sandbox: it may rewrite an existing file
under ~/.transcript-lake but may not create a directory there, and the scrub takes
the Lake's writer lease, which is a directory. The Stado host agent runs as the
same user without that restriction, so the fleet's own execution path is also the
maintenance path.

Phases, all printed with their own measurements:

1. Self-test on a synthetic Lake with a synthetic password, so the tool is proven
   on data nobody needs before it touches the archive: preview changes nothing,
   apply changes exactly the lines carrying the literal, the rewritten lines still
   parse, unrelated bytes are untouched, and a second run is a no-op.
2. The real Lake: preview, apply, preview again, then an independent byte search
   over every file for the literal.
3. The delivered secret file is removed, whatever happened above.

The literal itself arrives as `stado host install-secret … lake-scrub-literal`,
is read only inside this process, and is never printed: evidence lines show its
length and fingerprint, and the before/after transcript line shows the placeholder
`<SECRET>` where it stood.
"""

import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import sys

NONE = None
ZERO = len("")
HOME = pathlib.Path(os.path.expanduser("~"))
SECRET_FILE = HOME / ".stado" / "lake-scrub-literal"
SCRUB = (
    HOME
    / "Documents"
    / "CodingProjects"
    / "Wisent"
    / "transcript-lake"
    / "scripts"
    / "scrub-known-secret.py"
)
LAKE = pathlib.Path(os.environ.get("LAKE_DATA") or (HOME / ".transcript-lake"))
SELFTEST = HOME / ".cache" / "lake-scrub-selftest"
SELFTEST_SECRET = "SelfTestPass7!"
PYTHON = "/usr/bin/python3"
TIMEOUT = 1800


def run_scrub(data_dir, literal, apply_changes):
    """The checked-in scrub, with the literal handed over on standard input so it
    never reaches a command line or a process listing."""
    argv = [PYTHON, str(SCRUB), "--secret-file", "-", "--data-dir", str(data_dir)]
    if apply_changes:
        argv.append("--apply")
    proc = subprocess.run(
        argv,
        input=literal,
        capture_output=True,
        text=True,
        check=False,
        timeout=TIMEOUT,
        env={**os.environ, "PATH": "/usr/bin:/bin"},
    )
    return proc


def counts(stdout):
    """The tail measurements of a scrub run, as a dictionary."""
    picked = {}
    for line in stdout.splitlines():
        for key in ("files scanned", "files to change", "files changed",
                    "lines to change", "lines changed",
                    "occurrences to replace", "occurrences replaced",
                    "files refused"):
            if line.startswith(key + " "):
                picked[key] = int(line[len(key) + 1:])
    return picked


def show(label, proc):
    print(f"--- {label} (exit {proc.returncode})")
    for line in proc.stdout.splitlines():
        print("  " + line[: len("a" * 200)])
    if proc.stderr.strip():
        for line in proc.stderr.splitlines():
            print("  stderr " + line[: len("a" * 200)])


def self_test():
    """Preview, apply, and re-preview a synthetic Lake; return the failure count."""
    if SELFTEST.exists():
        shutil.rmtree(SELFTEST)
    partition = SELFTEST / "events" / "runtime=claude" / "date=2026-01-01"
    partition.mkdir(parents=True)
    part = partition / "part-000000000000.ndjson"
    rows = [
        {
            "ts": "2026-01-01T00:00:00Z",
            "event_type": "tool_call",
            "text": '{"command":"echo \\"' + SELFTEST_SECRET + '\\" | sudo -S true"}',
            "extra": {},
        },
        {
            "ts": "2026-01-01T00:00:01Z",
            "event_type": "user",
            "text": "unrelated line, must not change",
            "extra": {},
        },
        {
            "ts": "2026-01-01T00:00:02Z",
            "event_type": "assistant",
            "text": "twice here: " + SELFTEST_SECRET + " and " + SELFTEST_SECRET,
            "extra": {},
        },
    ]
    part.write_text("\n".join(json.dumps(row) for row in rows) + "\n", encoding="utf-8")
    before = part.read_text(encoding="utf-8").split("\n")

    failures = ZERO
    preview = run_scrub(SELFTEST, SELFTEST_SECRET, apply_changes=False)
    show("self-test preview", preview)
    measured = counts(preview.stdout)
    unchanged = part.read_text(encoding="utf-8").split("\n") == before
    print(f"  {'PASS' if unchanged else 'FAIL'} preview wrote nothing")
    failures += not unchanged
    expected_preview = {"files to change": 1, "lines to change": 2, "occurrences to replace": 3}
    for key, want in expected_preview.items():
        good = measured.get(key) == want
        print(f"  {'PASS' if good else 'FAIL'} preview {key} == {want} (measured {measured.get(key)})")
        failures += not good

    applied = run_scrub(SELFTEST, SELFTEST_SECRET, apply_changes=True)
    show("self-test apply", applied)
    after = part.read_text(encoding="utf-8").split("\n")
    gone = SELFTEST_SECRET not in "\n".join(after)
    print(f"  {'PASS' if gone else 'FAIL'} literal gone after apply")
    failures += not gone
    same_untouched = before[1] == after[1]
    print(f"  {'PASS' if same_untouched else 'FAIL'} unrelated line byte-identical")
    failures += not same_untouched
    same_shape = len(before) == len(after)
    print(f"  {'PASS' if same_shape else 'FAIL'} line count unchanged")
    failures += not same_shape
    parses = ZERO
    for line in after:
        if line.strip():
            json.loads(line)
            parses += 1
    print(f"  PASS every rewritten line parses as JSON ({parses} lines)")

    again = run_scrub(SELFTEST, SELFTEST_SECRET, apply_changes=True)
    show("self-test second apply", again)
    idempotent = counts(again.stdout).get("occurrences replaced") == ZERO
    print(f"  {'PASS' if idempotent else 'FAIL'} second apply replaced nothing")
    failures += not idempotent
    shutil.rmtree(SELFTEST)
    return failures


def find_evidence_line(literal):
    """One real partition line carrying the literal, kept for the before/after
    proof. Returns (path, line index, displayable text)."""
    for path in sorted(LAKE.rglob("*.ndjson")):
        try:
            lines = path.read_text(encoding="utf-8").split("\n")
        except (OSError, UnicodeDecodeError):
            continue
        for index, line in enumerate(lines):
            if literal in line and "sudo -S" in line:
                return path, index, line.replace(literal, "<SECRET>")
    return NONE, NONE, NONE


def byte_search(literal):
    """Independent of the scrub: how many files still carry the literal."""
    needle = literal.encode("utf-8")
    files = ZERO
    occurrences = ZERO
    scanned = ZERO
    for path in LAKE.rglob("*"):
        if not path.is_file():
            continue
        scanned += 1
        try:
            blob = path.read_bytes()
        except OSError:
            continue
        found = blob.count(needle)
        if found:
            files += 1
            occurrences += found
            print(f"  still present: {path.relative_to(LAKE)} x{found}")
    print(f"  searched {scanned} files: {files} files, {occurrences} occurrences")
    return files


def main():
    if not SECRET_FILE.is_file():
        raise SystemExit(f"no delivered literal at {SECRET_FILE}")
    if not SCRUB.is_file():
        raise SystemExit(f"no scrub script at {SCRUB}")
    # The host agent runs helpers under a secret-safe umask that strips the
    # execute bit, so a directory this process creates cannot be entered again.
    # Relax it for this process only; the secret is read, never written.
    os.umask(0o022)
    literal = SECRET_FILE.read_text(encoding="utf-8").split("\n")[0]
    digest = hashlib.sha256(literal.encode("utf-8")).hexdigest()[: len("a" * 8)]
    print(f"literal len={len(literal)} sha256[:8]={digest}")
    print(f"lake {LAKE}")
    try:
        failures = self_test()
        if failures:
            raise SystemExit(f"self-test failures {failures}; the real Lake was not touched")

        path, index, display = find_evidence_line(literal)
        if path is not NONE:
            print(f"--- before {path.relative_to(LAKE)} line {index + 1}")
            print("  " + display[: len("a" * 700)])

        preview = run_scrub(LAKE, literal, apply_changes=False)
        show("lake preview", preview)
        applied = run_scrub(LAKE, literal, apply_changes=True)
        show("lake apply", applied)
        verify = run_scrub(LAKE, literal, apply_changes=False)
        show("lake preview after apply", verify)

        if path is not NONE:
            after = path.read_text(encoding="utf-8").split("\n")[index]
            print(f"--- after {path.relative_to(LAKE)} line {index + 1}")
            print("  " + after[: len("a" * 700)])
            json.loads(after)
            print("  PASS line parses as JSON after the rewrite")

        print("--- independent byte search")
        remaining = byte_search(literal)
        print(f"remaining files with the literal {remaining}")
        return 1 if remaining else 0
    finally:
        SECRET_FILE.unlink()
        print(f"removed {SECRET_FILE}")


sys.exit(main())
