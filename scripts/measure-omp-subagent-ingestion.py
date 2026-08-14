#!/usr/bin/env python3
"""Measure what ingesting omp subagent transcripts costs, on real files.

Closing the subagent gap means the Lake will backfill every `<AgentName>.jsonl`
that was never discovered. Before that runs against the production Lake on a disk
with limited headroom, this replays TODAY's real agent transcripts into a throwaway
Lake and reports the numbers a decision needs: how many events each agent produces,
and how many bytes of Lake and Oko projection one byte of source becomes.

The source files are hard-linked, never copied, so the measurement adds no bytes
and cannot modify a transcript: `rebuild` only ever reads them.
"""

import json
import os
import pathlib
import shutil
import subprocess
import sys
import time

NONE = None
ZERO = len("")
HOME = pathlib.Path(os.path.expanduser("~"))
SOURCES = HOME / ".omp" / "agent" / "sessions"
BUILD = HOME / ".cache" / "transcript-lake-build" / "release" / "transcript-lake"
SCRATCH = HOME / ".cache" / "lake-subagent-measure"
TIMEOUT = 3600
DAY_SECONDS = 60 * 60 * 24


def tree_bytes(path):
    if not path.exists():
        return ZERO
    return sum(item.stat().st_size for item in path.rglob("*") if item.is_file())


def main():
    # The helper umask strips the execute bit, which would make every directory
    # created here unenterable, cargo-style.
    os.umask(0o022)
    if not BUILD.is_file():
        raise SystemExit(f"no build at {BUILD}; run check-masker first")
    cutoff = time.time() - DAY_SECONDS
    if SCRATCH.exists():
        shutil.rmtree(SCRATCH)
    mirror = SCRATCH / "home" / ".omp" / "agent" / "sessions"

    linked = ZERO
    source_bytes = ZERO
    for root in sorted(SOURCES.iterdir()):
        if not root.is_dir():
            continue
        for session_dir in sorted(root.iterdir()):
            if not session_dir.is_dir():
                continue
            fresh = [
                path
                for path in sorted(session_dir.rglob("*.jsonl"))
                if path.is_file() and path.stat().st_mtime >= cutoff
            ]
            if not fresh:
                continue
            # The session's own transcript comes along, because a subagent file is
            # only evidence in the context of the conversation that spawned it.
            owner = root / f"{session_dir.name}.jsonl"
            for path in ([owner] if owner.is_file() else []) + fresh:
                target = mirror / root.name / path.relative_to(root)
                target.parent.mkdir(parents=True, exist_ok=True)
                os.link(path, target)
                linked += 1
                source_bytes += path.stat().st_size
    print(f"hard-linked {linked} real transcripts, {source_bytes} source bytes")
    if not linked:
        raise SystemExit("no agent transcript was modified in the last day")

    current = SCRATCH / "current"
    current.mkdir(parents=True, exist_ok=True)
    target = SCRATCH / "replayed"
    proc = subprocess.run(
        [str(BUILD), "rebuild", "--to", str(target), "--source", "omp"],
        capture_output=True,
        text=True,
        check=False,
        timeout=TIMEOUT,
        env={
            **os.environ,
            "HOME": str(SCRATCH / "home"),
            "LAKE_DATA": str(current),
            "PATH": "/usr/bin:/bin",
        },
    )
    if proc.returncode != ZERO:
        print(proc.stdout.strip()[: len("a" * 400)])
        print(proc.stderr.strip()[: len("a" * 400)])
        raise SystemExit(f"replay exit {proc.returncode}")
    summary = json.loads(proc.stdout)
    print(f"replay: {json.dumps(summary, sort_keys=True)[: len('a' * 400)]}")

    per_session = {}
    per_file = {}
    for part in sorted((target / "events").rglob("part-*.ndjson")):
        # Partition lines are newline-delimited JSON. `str.splitlines()` would also
        # split on U+2028 and friends, which real transcript text contains, and each
        # half would then fail to parse.
        for line in part.read_text(encoding="utf-8", errors="replace").split("\n"):
            if not line.strip():
                continue
            row = json.loads(line)
            per_session[row.get("session_id")] = per_session.get(row.get("session_id"), ZERO) + 1
        per_file[part.name] = per_file.get(part.name, ZERO) + part.stat().st_size
    events_bytes = tree_bytes(target / "events")
    export_bytes = tree_bytes(target / "exports")
    print(f"events bytes {events_bytes}  oko export bytes {export_bytes}"
          f"  total/source {round((events_bytes + export_bytes) / max(source_bytes, 1), 3)}")
    print(f"sessions in the replayed Lake: {len(per_session)}")
    for session, count in sorted(per_session.items(), key=lambda item: -item[1]):
        print(f"  {session} events={count}")
    return ZERO


sys.exit(main())
