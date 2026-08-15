#!/usr/bin/env python3
"""Standing repair: keep every value Skarbiec holds out of the transcript archive.

Masking catches secret-bearing SHAPES at write time. It cannot catch a human typing
a password into a chat as prose, and it cannot catch a transcript imported years
later. The vault is the one place that knows what the fleet's secrets actually are,
so this job asks it, and rewrites any value it finds in the Lake to the masker's own
`[masked:credential:…]` marker.

How it stays safe:

- Values are read into memory and never printed, written to a file, or passed on a
  command line: the literal list reaches the scrub on standard input, and only
  lengths, digests and counts are logged.
- A literal is only material if it can plausibly be a secret in text: at least
  twelve characters, at least three of the four character classes, no newline (a
  PEM cannot match the escaped form stored in a partition anyway), and never the
  `username` field. Everything rejected is counted, never named.
- The scrub itself counts before it writes and refuses any literal that occurs in
  more files than its cap, so one wrong value in the vault costs a report rather
  than the archive.
- Idempotent: a run that finds nothing rewrites nothing and reports zero.

Residual, stated plainly because it is the point: a secret that was never put in
the vault cannot be scrubbed by this job. The archive is exactly as clean as the
vault is complete, which is the argument for putting credentials in Skarbiec rather
than in a chat message.
"""

import hashlib
import json
import os
import pathlib
import subprocess
import sys
import time

NONE = None
ZERO = len("")
HOME = pathlib.Path(os.environ.get("HOME") or os.path.expanduser("~"))
SKARBIEC = HOME / ".stado" / "bin" / "skarbiec"
REPO_SCRUB = (
    HOME
    / "Documents"
    / "CodingProjects"
    / "Wisent"
    / "transcript-lake"
    / "scripts"
    / "scrub-known-secret.py"
)

PYTHON = "/usr/bin/python3"
LAKE = pathlib.Path(os.environ.get("LAKE_DATA") or (HOME / ".transcript-lake"))
# Both vaults this host owns, named explicitly: ~/.stado holds around twenty
# *vault*.json files, most of them pre-migration snapshots, and scrubbing against a
# stale one would both miss current secrets and resurrect retired ones.
VAULTS = (
    HOME / ".stado" / "skarbiec.vault.json",
    HOME / ".stado" / "weles-skarbiec.vault.json",
)
# Fields that are identifiers rather than secrets. Everything else an item carries
# is treated as material, because the fleet's secrets live in free-form kinds
# (bundle, stado-secret, internal-authority) whose field names nobody maintains a
# list of.
IDENTIFIER_FIELDS = ("username", "user", "login", "email", "account", "id", "url", "host")
MIN_LENGTH = len("a" * 12)
MIN_CLASSES = len("abc")
# The inner scrub now waits for the streamer's writer lease before it rewrites
# anything, and a backfill can hold that lease for hours; on top of the wait it
# still has to sweep several gigabytes. A timeout shorter than wait + sweep
# turns every scheduled run into a killed process that reports nothing, which
# is how this job spent its first hours. launchd will not start a second copy
# while one runs, so a long ceiling costs nothing.
TIMEOUT = 4 * 3600
# Where a scheduled run remembers how far it got. Not in the Lake: the Lake is the
# thing being repaired, and its own tooling deletes derived files.
STATE = (
    pathlib.Path(os.environ.get("HOME") or os.path.expanduser("~"))
    / "Library"
    / "Application Support"
    / "wisent-transcript-lake-scrub"
    / "state.json"
)
# A run covers what changed since the previous one, with a small overlap for clock
# and mtime granularity, and sweeps everything once a day because the literal list
# grows when the vault does.
OVERLAP_SECONDS = 60 * 10
FULL_SWEEP_SECONDS = 60 * 60 * 24


def resolve_scrub():
    """Where the scrub script is, in the order a caller can rely on.

    A LaunchAgent cannot read ~/Documents: macOS denies it with `Operation not
    permitted` and no prompt, because the job holds no Documents grant. So the
    installed copy sits beside this file outside ~/Documents and wins, and the
    repository path is the fallback for an operator running this by hand.
    """
    named = os.environ.get("LAKE_SCRUB_SCRIPT")
    sibling = pathlib.Path(__file__).resolve().parent / "scrub-known-secret.py"
    for candidate in (pathlib.Path(named) if named else NONE, sibling, REPO_SCRUB):
        if candidate is not NONE and candidate.is_file():
            return candidate
    return REPO_SCRUB


def digest(value):
    return hashlib.sha256(value.encode("utf-8")).hexdigest()[: len("a" * 8)]


def classes(value):
    """How many of lower, upper, digit, symbol the value draws on."""
    return sum(
        (
            any(character.islower() for character in value),
            any(character.isupper() for character in value),
            any(character.isdigit() for character in value),
            any(not character.isalnum() for character in value),
        )
    )


def material(value):
    """Whether a stored value could occur as a secret inside transcript text."""
    return (
        isinstance(value, str)
        and len(value) >= MIN_LENGTH
        and "\n" not in value
        and "\r" not in value
        and classes(value) >= MIN_CLASSES
    )


def run(argv, vault, stdin=NONE):
    return subprocess.run(
        argv,
        input=stdin,
        capture_output=True,
        text=True,
        check=False,
        timeout=TIMEOUT,
        env={
            **os.environ,
            "HOME": str(HOME),
            "SKARBIEC_VAULT_FILE": str(vault),
            "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin",
        },
    )


def collect(vault):
    """Every value in one vault that could be a secret in text, with coverage
    reported per kind and every item that contributed nothing named.

    `list` returns envelope metadata only, so it is safe to log; `get` returns
    decrypted fields, so its output is consumed here and never surfaces."""
    if not vault.is_file():
        print(f"vault {vault} absent")
        return set()
    listing = run([str(SKARBIEC), "list"], vault)
    if listing.returncode != ZERO:
        print(f"vault {vault} unreadable: {listing.stderr.strip().splitlines()[-1:]}")
        return set()
    items = json.loads(listing.stdout)
    values = set()
    per_kind = {}
    skipped = []
    unreadable = []
    for item in items:
        if item.get("deleted"):
            continue
        # A Weles-managed item can carry no kind at all, and `None` does not sort.
        kind = item.get("kind") or "unknown"
        counters = per_kind.setdefault(
            kind, {"items": ZERO, "contributed": ZERO, "values": ZERO, "rejected": ZERO}
        )
        counters["items"] += 1
        read = run([str(SKARBIEC), "get", item["id"]], vault)
        if read.returncode != ZERO:
            unreadable.append((item["id"], kind, "unreadable"))
            continue
        try:
            document = json.loads(read.stdout)
        except ValueError:
            unreadable.append((item["id"], kind, "not JSON"))
            continue
        fields = document.get("fields")
        if not isinstance(fields, dict):
            skipped.append((item["id"], kind, "no fields"))
            continue
        contributed = ZERO
        reasons = []
        for name, value in fields.items():
            if name.lower() in IDENTIFIER_FIELDS:
                reasons.append(f"{name}:identifier")
                continue
            if material(value):
                values.add(value)
                contributed += 1
            else:
                counters["rejected"] += 1
                reasons.append(
                    f"{name}:"
                    + (
                        "empty"
                        if not isinstance(value, str) or not value
                        else "multiline"
                        if "\n" in value or "\r" in value
                        else f"len<{MIN_LENGTH}"
                        if len(value) < MIN_LENGTH
                        else f"classes<{MIN_CLASSES}"
                    )
                )
        counters["values"] += contributed
        if contributed:
            counters["contributed"] += 1
        else:
            skipped.append((item["id"], kind, ",".join(reasons) or "no fields"))
    print(f"vault {vault.name}")
    # Some stored items carry no kind at all, and a set with None in it cannot
    # be sorted against strings -- the fault that made this job exit 1 every
    # hour while reporting nothing. An unkinded item is still material.
    for kind in sorted(per_kind, key=lambda name: name or ""):
        counters = per_kind[kind]
        print(
            f"  kind {(kind or 'unkinded'):20s} items={counters['items']:4d}"
            f" contributed={counters['contributed']:4d} values={counters['values']:4d}"
            f" fields_rejected={counters['rejected']:4d}"
        )
    print(f"  items contributing nothing {len(skipped)}, unreadable {len(unreadable)}")
    for item_id, kind, reason in (skipped + unreadable)[: len("a" * 40)]:
        print(f"    skipped {item_id} kind={kind} why={reason[: len('a' * 90)]}")
    if len(skipped) + len(unreadable) > len("a" * 40):
        print(f"    ... and {len(skipped) + len(unreadable) - len('a' * 40)} more")
    return values


def read_state():
    """When the last run and the last full sweep happened. A missing or damaged
    state file means: sweep everything, which is the safe direction."""
    try:
        return json.loads(STATE.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}


def write_state(state):
    STATE.parent.mkdir(parents=True, exist_ok=True)
    temporary = STATE.with_name(f"{STATE.name}.tmp-{os.getpid()}")
    temporary.write_text(json.dumps(state, sort_keys=True), encoding="utf-8")
    os.replace(temporary, STATE)


def main():
    started = time.time()
    print(f"started {time.strftime('%Y-%m-%dT%H:%M:%S%z')}")
    scrub_script = resolve_scrub()
    if not scrub_script.is_file():
        raise SystemExit(f"no scrub script at {scrub_script}")
    print(f"scrub {scrub_script} sha256[:8]={digest(scrub_script.read_text(encoding='utf-8'))}")
    if not SKARBIEC.is_file():
        raise SystemExit(f"no skarbiec binary at {SKARBIEC}")
    if not LAKE.is_dir():
        raise SystemExit(f"no Lake at {LAKE}")

    literals = set()
    for vault in VAULTS:
        literals |= collect(vault)
    print(f"literals from the vaults {len(literals)}")
    for value in sorted(literals, key=digest):
        print(f"  literal len={len(value)} sha256[:8]={digest(value)}")
    if not literals:
        print("nothing to scrub; the vaults hold no value that could occur in text")
        print(f"finished in {time.time() - started:.1f}s")
        return ZERO

    # Incremental by default and complete once a day: partitions are append-only, so
    # only a file written since the last pass can have gained a secret, but a full
    # sweep still has to happen because the literal list itself changes when the
    # vault does, and a new item's value may sit in an old partition.
    state = read_state()
    last_full = float(state.get("last_full_sweep", ZERO))
    full = time.time() - last_full > FULL_SWEEP_SECONDS
    since = ZERO if full else max(float(state.get("last_run", ZERO)) - OVERLAP_SECONDS, ZERO)
    print(f"sweep {'full' if full else 'incremental'} since={since:.0f}")

    scrub = run(
        [
            PYTHON,
            str(scrub_script),
            "--secret-file",
            "-",
            "--data-dir",
            str(LAKE),
            "--since",
            str(int(since)),
            "--apply",
        ],
        VAULTS[ZERO],
        stdin="\n".join(sorted(literals)),
    )
    for line in scrub.stdout.splitlines():
        print("  " + line[: len("a" * 200)])
    for line in scrub.stderr.splitlines():
        print("  stderr " + line[: len("a" * 200)])
    print(f"scrub exit {scrub.returncode}")
    busy = "state writer lock is held" in scrub.stdout + scrub.stderr
    if busy:
        # The streamer holds the lease for the length of a catch-up. Nothing was
        # rewritten, so the next run must cover the same window again.
        print("lake busy: the streamer holds the writer lease; state not advanced")
    else:
        write_state(
            {
                "last_run": started,
                "last_full_sweep": started if full else last_full,
                "literals": len(literals),
            }
        )
        print(f"state {STATE}")
    print(f"finished in {time.time() - started:.1f}s")
    # A refused literal (over its file cap) is a report, not a failure of the run:
    # exit non-zero so the operator sees it, after the accepted ones were applied.
    return 75 if busy else scrub.returncode


sys.exit(main())
