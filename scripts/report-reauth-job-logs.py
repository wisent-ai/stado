#!/usr/bin/env python3
"""Show what each scheduled reauth job wrote on its own last tick.

Running a runner by hand proves the code; only the job's own log proves the
schedule. The log paths come from the loaded unit rather than a guess, because
the two have disagreed before.

Read-only. Prints the tail of each stream.
"""

import os
import pathlib
import plistlib
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
DAEMONS = pathlib.Path("/Library/LaunchDaemons")
LABELS = (
    "com.wisent.codex-reauth",
    "com.wisent.claude-reauth",
    "com.wisent.kimi-reauth",
)
TAIL = len("l" * 8)


def tail(path):
    try:
        lines = [line for line in path.read_text(encoding="utf-8", errors="replace").splitlines() if line.strip()]
    except OSError as error:
        return [f"(unreadable: {error})"]
    return lines[-TAIL:] or ["(empty)"]


def main():
    for label in LABELS:
        path = DAEMONS / f"{label}.plist"
        print(f"== {label}")
        if not path.is_file():
            print("  no system unit")
            continue
        document = plistlib.loads(path.read_bytes())
        for key in ("StandardOutPath", "StandardErrorPath"):
            value = document.get(key)
            if not value:
                continue
            stream = pathlib.Path(value)
            stamp = ""
            if stream.is_file():
                import datetime

                stamp = datetime.datetime.fromtimestamp(
                    stream.stat().st_mtime, datetime.timezone.utc
                ).isoformat()
            print(f"  {key} {stream} {stamp}")
            if stream.is_file():
                for line in tail(stream):
                    print(f"    {line[: len('a' * 180)]}")
    return NONE


sys.exit(main())
