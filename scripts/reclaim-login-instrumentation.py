#!/usr/bin/env python3
"""Delete the oversized instrumentation dumps a login run leaves behind.

Each browser session writes `<label>_<stamp>.inst.json` beside its recording, and
on this host those run to a gigabyte apiece. The janitor cannot help within a
day -- the schema floors retention at 86400 seconds -- and the disk fell to under
three gigabytes free, which is where Chromium stops starting and every login
fails for a reason that has nothing to do with logging in.

The video and the DOM snapshot of each run are kept: they are what a failure is
diagnosed from. Only the instrumentation dumps go, and only those above the size
that makes them a problem.
"""

import os
import pathlib
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
ROOTS = (
    HOME / "weles" / "recordings",
    HOME / ".local" / "share" / "weles-worker",
)
PATTERN = "*.inst.json"
BIG = int("50000000")


def free_bytes():
    stat = os.statvfs(str(HOME))
    return stat.f_bavail * stat.f_frsize


def main():
    print(f"free before {free_bytes()}")
    seen = set()
    freed = ZERO
    for root in ROOTS:
        if not root.is_dir():
            continue
        for path in root.rglob(PATTERN):
            resolved = path.resolve()
            if resolved in seen or not path.is_file():
                continue
            seen.add(resolved)
            size = path.stat().st_size
            if size < BIG:
                continue
            try:
                path.unlink()
            except OSError as error:
                print(f"  kept {path}: {error}")
                continue
            freed += size
            print(f"  removed {size:>12}  {path}")
    print(f"freed       {freed}")
    print(f"free after  {free_bytes()}")
    return NONE


sys.exit(main())
