#!/usr/bin/env python3
"""Locate the Weles worker tree and its reauth runners on this host.

Repairing a runner means delivering files to the tree the host actually runs,
and that tree is a versioned service directory rather than a checkout. This
prints the roots that exist, the runner paths inside them, and which launchd
jobs point at those paths.

Read-only.
"""

import os
import pathlib
import plistlib
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
RUNNERS = (
    "scripts/trajectories/codex/reauth.mjs",
    "scripts/trajectories/claude/reauth.mjs",
    "scripts/trajectories/_shared/reauth_config.mjs",
)
UNIT_ROOTS = (pathlib.Path("/Library/LaunchDaemons"), HOME / "Library" / "LaunchAgents")


def candidate_roots():
    roots = []
    services = HOME / ".stado" / "services"
    if services.is_dir():
        for entry in sorted(services.glob("weles*/*/*")):
            if entry.is_dir():
                roots.append(entry)
    for extra in (HOME / "weles", HOME / "Documents" / "weles"):
        if extra.is_dir():
            roots.append(extra)
    return roots


def main():
    roots = candidate_roots()
    if not roots:
        print("no weles tree found under ~/.stado/services or ~")
    for root in roots:
        present = [name for name in RUNNERS if (root / name).is_file()]
        if not present:
            continue
        print(f"root {root}")
        for name in RUNNERS:
            path = root / name
            print(f"  {name:<48} {'present' if path.is_file() else 'absent'}")

    print("launchd jobs naming a reauth runner:")
    for unit_root in UNIT_ROOTS:
        if not unit_root.is_dir():
            continue
        for path in sorted(unit_root.glob("*.plist")):
            try:
                document = plistlib.loads(path.read_bytes())
            except (OSError, ValueError):
                continue
            arguments = " ".join(str(value) for value in document.get("ProgramArguments", []))
            if "reauth" in arguments or "reauth" in path.stem:
                print(f"  {document.get('Label', path.stem):<40} {arguments[: len('a' * 90)]}")
    return NONE


sys.exit(main())
