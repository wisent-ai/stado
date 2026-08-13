#!/usr/bin/env python3
"""Run each reauth job now, through launchd, and report what it did.

A job whose interval is two hours cannot be verified by waiting, and running the
runner by hand proves the code rather than the schedule. `launchctl kickstart`
starts the loaded unit itself, so the evidence lands in the job's own log with
the job's own environment.

Prints each unit's state and the tail of its log after the run.
"""

import os
import pathlib
import plistlib
import re
import subprocess
import sys
import time

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
DAEMONS = pathlib.Path("/Library/LaunchDaemons")
LABELS = (
    "com.wisent.codex-reauth",
    "com.wisent.claude-reauth",
    "com.wisent.kimi-reauth",
)
SETTLE = len("s" * 25)
TAIL = len("l" * 6)


def run(*args):
    proc = subprocess.run(args, capture_output=True, text=True, check=False)
    return proc.stdout + proc.stderr


def log_paths(label):
    path = DAEMONS / f"{label}.plist"
    if not path.is_file():
        return []
    document = plistlib.loads(path.read_bytes())
    seen = []
    for key in ("StandardOutPath", "StandardErrorPath"):
        value = document.get(key)
        if value and value not in seen:
            seen.append(value)
    return [pathlib.Path(value) for value in seen]


def main():
    marks = {}
    for label in LABELS:
        for path in log_paths(label):
            marks[path] = path.stat().st_size if path.is_file() else ZERO
        detail = run("/usr/bin/sudo", "-n", "/bin/launchctl", "kickstart", f"system/{label}")
        print(f"{label:<26} kickstart {detail.strip() or 'ok'}")

    time.sleep(SETTLE)
    for label in LABELS:
        print(f"== {label}")
        printed = run("/usr/bin/sudo", "-n", "/bin/launchctl", "print", f"system/{label}")
        exit_code = re.search(r"^\s*last exit code = (.+)$", printed, re.MULTILINE)
        print(f"  last exit code {exit_code.group(len(['v'])).strip() if exit_code else '(none)'}")
        for path in log_paths(label):
            if not path.is_file():
                continue
            with path.open("r", encoding="utf-8", errors="replace") as handle:
                handle.seek(marks.get(path, ZERO))
                fresh = [line.rstrip() for line in handle if line.strip()]
            for line in fresh[-TAIL:] or ["(nothing new)"]:
                print(f"    {line[: len('a' * 180)]}")
            break
    return NONE


sys.exit(main())
