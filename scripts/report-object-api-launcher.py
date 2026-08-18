#!/usr/bin/env python3
"""Print this host's stado-object-api-launcher with secret values masked.

Read-only companion to report-object-api-host.py: before the object API is
restarted, the operator must know what its launcher resolves at startup —
which skarbiec it asks, which port it binds, which files it reads — because a
restart is a bet that the process can be recreated, and the launcher is where
that bet is written down. Secret-looking assignments are masked, never printed.
"""

import os
import pathlib
import re

HOME = pathlib.Path(os.path.expanduser("~"))
LAUNCHER = HOME / ".stado" / "bin" / "stado-object-api-launcher"
MASK = re.compile(r"(?i)^(\s*(?:export\s+)?\w*(?:TOKEN|SECRET|UNLOCK|PASSWORD)\w*=).*")


def main():
    if not LAUNCHER.is_file():
        print(f"(no launcher at {LAUNCHER})")
        return
    print(f"== {LAUNCHER}")
    for line in LAUNCHER.read_text().splitlines():
        print(MASK.sub(r"\1<masked>", line))


main()
