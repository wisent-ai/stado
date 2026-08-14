#!/usr/bin/env python3
"""List the artifacts a login run left behind, newest first.

The Weles trajectories record video and DOM for exactly the failure that is now
in the way, and the repository says in capitals that those are what a failure is
diagnosed from. They are useless if nobody can find them, so print the recent
ones with their sizes and ages, per run directory.

Read-only.
"""

import datetime
import os
import pathlib
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
ROOTS = (
    HOME / ".local" / "share" / "weles-worker",
    HOME / "weles" / "var",
    HOME / ".local" / "state" / "weles",
)
INTERESTING = (".webm", ".mp4", ".html", ".png", ".json", ".log", ".txt")
RECENT = len("h" * 12)
LIMIT = len("l" * 25)


def age_hours(path):
    return (datetime.datetime.now().timestamp() - path.stat().st_mtime) / float(len("s" * 3600))


def main():
    found = []
    for root in ROOTS:
        if not root.is_dir():
            print(f"root {root} absent")
            continue
        print(f"root {root}")
        for path in root.rglob("*"):
            if not path.is_file() or path.suffix.lower() not in INTERESTING:
                continue
            hours = age_hours(path)
            if hours <= RECENT:
                found.append((hours, path))
    for hours, path in sorted(found)[:LIMIT]:
        print(f"  {hours:6.2f}h  {path.stat().st_size:>10}  {path}")
    if not found:
        print("  no artifact newer than the last twelve hours")
    return NONE


sys.exit(main())
