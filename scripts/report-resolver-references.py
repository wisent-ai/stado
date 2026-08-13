#!/usr/bin/env python3
"""Find everything on this host that can start the resolver.

A resolver reappears seconds after being ended while launchd's own record says
the job is not running, so something else starts it and exits. Anything that
does so competes with the managed unit for the same ports and makes every
repair temporary. This lists unit files and owner-only helpers whose program
mentions the resolver, with the label or path that would run it.

Read-only.
"""

import os
import pathlib
import plistlib
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
UNIT_ROOTS = (pathlib.Path("/Library/LaunchDaemons"), HOME / "Library" / "LaunchAgents")
SCRIPT_ROOTS = (HOME / ".stado" / "bin",)
NEEDLE = "resolver"
MANAGED = "com.wisent.stado-resolver"


def main():
    print("unit files naming the resolver:")
    for root in UNIT_ROOTS:
        if not root.is_dir():
            continue
        for path in sorted(root.glob("*.plist")):
            try:
                document = plistlib.loads(path.read_bytes())
            except (OSError, ValueError):
                continue
            arguments = " ".join(str(value) for value in document.get("ProgramArguments", []))
            if NEEDLE not in arguments and NEEDLE not in str(document.get("Program", "")):
                continue
            label = document.get("Label", path.stem)
            mark = "  (the managed one)" if label == MANAGED else ""
            print(f"  {label:<50} {arguments[: len('a' * 70)]}{mark}")

    print("helpers naming the resolver:")
    for root in SCRIPT_ROOTS:
        if not root.is_dir():
            continue
        for path in sorted(root.iterdir()):
            if not path.is_file() or path.is_symlink():
                continue
            try:
                text = path.read_text(encoding="utf-8", errors="ignore")
            except OSError:
                continue
            if f"{NEEDLE} serve" in text or "start-resolver" in text:
                print(f"  {path}")
    return NONE


sys.exit(main())
