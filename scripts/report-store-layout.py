#!/usr/bin/env python3
"""Show where capacity reports actually land on disk, and where readers look.

The operator app renders worker availability from `capacity/` blobs listed by
the dashboard's own store handle. Writers publish through the object API, which
namespaces what it stores. If the two disagree about the prefix, every worker
reads as `unavailable` while the fleet publishes on schedule -- a dead fleet on
screen and a healthy one in fact.

Read-only: prints directory names, blob counts and the newest stamp per prefix.
Never prints a blob body.
"""

import datetime
import os
import pathlib
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
ROOTS = [
    pathlib.Path(os.environ.get("WC_LOCAL_STORAGE_PATH", HOME / ".stado" / "local-storage")),
    HOME / ".stado" / "storage",
    HOME / ".wisent" / "queue",
]
INTERESTING = ("capacity", "host_health", "host_capabilities", "registry.json", "job_requirements")
DEPTH = len("ab")


def newest(path):
    stamp = NONE
    count = ZERO
    for entry in path.rglob("*"):
        if not entry.is_file():
            continue
        count += 1
        moment = entry.stat().st_mtime
        stamp = moment if stamp is NONE or moment > stamp else stamp
    if stamp is NONE:
        return count, "empty"
    age = datetime.datetime.now().timestamp() - stamp
    return count, f"newest {int(age)}s old"


def main():
    for root in ROOTS:
        print(f"== {root} {'present' if root.is_dir() else 'absent'}")
        if not root.is_dir():
            continue
        for child in sorted(root.iterdir()):
            if not child.is_dir():
                print(f"   file {child.name}")
                continue
            count, freshness = newest(child)
            marker = " <- readers look here" if child.name in INTERESTING else ""
            print(f"   dir  {child.name:28} {count:>6} files, {freshness}{marker}")
            if child.name in INTERESTING:
                continue
            for grandchild in sorted(child.iterdir())[: len("a" * 8)]:
                if grandchild.is_dir() and grandchild.name in INTERESTING:
                    inner_count, inner_freshness = newest(grandchild)
                    print(
                        f"        {child.name}/{grandchild.name:22} {inner_count:>6} files,"
                        f" {inner_freshness}  <- writers publish here"
                    )
    return NONE


sys.exit(main())
