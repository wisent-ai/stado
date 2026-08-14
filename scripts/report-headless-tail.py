#!/usr/bin/env python3
"""Print the last thing the browser logged before it stopped making progress.

The head of the log shows a healthy start: field trials, proxy config, six
children. The stall is at the other end, and the last line before silence names
the subsystem that never answered.
"""

import os
import pathlib
import sys

NONE = None
HOME = pathlib.Path(os.path.expanduser("~"))
LOG = HOME / ".local" / "state" / "weles" / "headless-verbose.log"
TAIL = 30


def main():
    if not LOG.is_file():
        raise SystemExit(f"no log at {LOG}")
    lines = [line.rstrip() for line in LOG.read_text(encoding="utf-8", errors="replace").splitlines() if line.strip()]
    print(f"log {len(lines)} lines from {LOG}")
    for line in lines[-TAIL:]:
        print(f"   {line[: len('a' * 165)]}")
    return NONE


sys.exit(main())
