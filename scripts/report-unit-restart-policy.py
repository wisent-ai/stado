#!/usr/bin/env python3
"""Say which always-on units actually come back by themselves.

"Always-on" is a name, not a mechanism: launchd restarts a job only when the
unit says `KeepAlive`, and a unit without it stays dead after its first crash
while `service show` reports the last exit as clean. Brama went down that way
and nothing noticed until a request failed.

Read-only: it reads unit files and reports.
"""

import os
import pathlib
import plistlib
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
ROOTS = (pathlib.Path("/Library/LaunchDaemons"), HOME / "Library" / "LaunchAgents")
PREFIX = "com.wisent."


def describe(path):
    try:
        document = plistlib.loads(path.read_bytes())
    except (OSError, ValueError) as error:
        return f"{path.name:<52} unreadable: {error}"
    keep = document.get("KeepAlive", False)
    run_at_load = document.get("RunAtLoad", False)
    interval = document.get("StartInterval")
    if interval:
        verdict = f"ticks every {interval}s"
    elif keep:
        verdict = "restarts on exit"
    elif run_at_load:
        verdict = "starts once, stays dead after a crash"
    else:
        verdict = "on demand"
    return f"{document.get('Label', path.stem):<52} {verdict}"


def main():
    for root in ROOTS:
        if not root.is_dir():
            continue
        for path in sorted(root.glob(f"{PREFIX}*.plist")):
            print(f"{root.name:<16} {describe(path)}")
    return NONE


sys.exit(main())
