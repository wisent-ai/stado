#!/usr/bin/env python3
"""Remove queue-job workdirs whose job can no longer use them.

Every claimed job gets `/tmp/wc-<job_id>/`; a failed or interrupted build
leaves multi-gigabyte `source/` and `.wisent-output/` trees there forever,
because workdir reaping is policy-owned and no declared cleaner names these
paths. On 2026-08-19 they filled the linux builder to 0 GiB free, its agent
failed admission closed, and the whole release pipeline starved on a host
that looked merely busy.

Safety boundary: only `/tmp/wc-*` directories older than MIN_AGE_HOURS are
removed — a running job touches its workdir constantly, so age is the
conservative proxy for "terminal" that needs no store access from the host.
Bootstrap scratch (`/tmp/stado-bootstrap-*`) is included: it exists only as
debris of one-off provisioning builds. Prints every removal with its size.
"""

import os
import pathlib
import shutil
import time

MIN_AGE_HOURS = 2
ROOTS = ["/tmp"]
PREFIXES = ("wc-", "stado-bootstrap-")


def tree_bytes(path: pathlib.Path) -> int:
    total = 0
    for directory, _, names in os.walk(path, onerror=lambda _: None):
        for name in names:
            try:
                total += (pathlib.Path(directory) / name).lstat().st_size
            except OSError:
                continue
    return total


def main():
    horizon = time.time() - MIN_AGE_HOURS * 3600
    freed = 0
    for root in ROOTS:
        base = pathlib.Path(root)
        if not base.is_dir():
            continue
        for entry in sorted(base.iterdir()):
            if not entry.name.startswith(PREFIXES) or not entry.is_dir():
                continue
            try:
                newest = max(
                    entry.stat().st_mtime,
                    max(
                        (p.stat().st_mtime for p in entry.rglob("*") if p.is_file()),
                        default=0.0,
                    ),
                )
            except OSError:
                newest = entry.stat().st_mtime
            if newest > horizon:
                print(f"kept {entry} (active within {MIN_AGE_HOURS}h)")
                continue
            size = tree_bytes(entry)
            shutil.rmtree(entry, ignore_errors=True)
            freed += size
            print(f"removed {entry} ({size / 2**30:.1f} GiB)")
    print(f"freed {freed / 2**30:.1f} GiB total")


main()
