#!/usr/bin/env python3
"""Name the paths this host has written in the last few minutes.

`capacity/` on the authority host is days old while the object API answers with
capacity reports seconds old, so the two are not the same bytes. Rather than
reason about which layout is current, look at what is actually being written and
where -- the newest files name the writer's layout, and the reader's empty
prefix names the mismatch.
"""

import datetime
import os
import pathlib
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
ROOT = pathlib.Path(os.environ.get("WC_LOCAL_STORAGE_PATH", HOME / ".stado" / "local-storage"))
WINDOW = float(os.environ.get("FRESH_WINDOW_SECONDS", 600))
SHOW = len("a" * 25)


def main():
    if not ROOT.is_dir():
        raise SystemExit(f"no store at {ROOT}")
    now = datetime.datetime.now().timestamp()
    fresh = []
    for entry in ROOT.rglob("*"):
        if not entry.is_file():
            continue
        age = now - entry.stat().st_mtime
        if age <= WINDOW:
            fresh.append((age, entry.relative_to(ROOT)))
    fresh.sort()
    print(f"store {ROOT}")
    print(f"files written in the last {int(WINDOW)}s: {len(fresh)}")
    for age, path in fresh[:SHOW]:
        print(f"   {int(age):>5}s  {str(path)[: len('a' * 110)]}")
    prefixes = {}
    for age, path in fresh:
        prefixes[path.parts[ZERO]] = prefixes.get(path.parts[ZERO], ZERO) + 1
    print("by top-level prefix: " + ", ".join(f"{name}={count}" for name, count in sorted(prefixes.items())))
    return NONE


sys.exit(main())
