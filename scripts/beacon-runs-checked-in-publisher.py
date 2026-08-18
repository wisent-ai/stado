#!/usr/bin/env python3
"""Run the beacon publisher that is checked in, not a copy that fell behind.

The authority's loop called `host_health_beacon_macos_daemon.sh`, a private older
copy whose label list is a hardcoded fallback:

    LABELS="${WC_HEALTH_UNITS:-com.wisent.skarbiec com.wisent.host-health-beacon}"

It never reads the registry, so this host published two labels out of the fourteen
it declares and `registry doctor` answered with a `missing-plist` row for each of
the rest -- services that are installed and running. The repository's
`deploy/host_health_beacon_macos.sh` derives the list from the registry and
matches the target by name or declared hostname; every other host runs it.

Only the file name is rewritten, leaving the surrounding quoting untouched: an
earlier attempt replaced the whole word including the opening quote and produced a
launcher that would not parse. The reload is left to the caller so the running
interpreter is replaced rather than re-reading an edited file.
"""

import os
import pathlib
import subprocess
import sys

NONE = None
HOME = pathlib.Path(os.path.expanduser("~"))
LAUNCHER = HOME / ".stado" / "bin" / "host-health-beacon-launcher"
STALE = "host_health_beacon_macos_daemon.sh"
WANTED = "host_health_beacon_macos.sh"


def main():
    if not LAUNCHER.is_file():
        raise SystemExit(f"no beacon launcher at {LAUNCHER}")
    installed = HOME / ".stado" / "bin" / WANTED
    if not installed.is_file():
        raise SystemExit(f"install the checked-in publisher first: {installed} is absent")
    original = LAUNCHER.read_text(encoding="utf-8")
    print(f"launcher   {LAUNCHER}")
    if STALE not in original:
        print(f"settled    already runs {WANTED}")
        return NONE
    body = original.replace(STALE, WANTED)
    LAUNCHER.with_name(LAUNCHER.name + ".before-publisher").write_text(original, encoding="utf-8")
    LAUNCHER.write_text(body, encoding="utf-8")
    check = subprocess.run(["/bin/sh", "-n", str(LAUNCHER)], capture_output=True,
                           text=True, check=False)
    if check.returncode != 0:
        LAUNCHER.write_text(original, encoding="utf-8")
        raise SystemExit(f"edit refused, launcher restored: {check.stderr.strip()}")
    print(f"before     {STALE}")
    print(f"after      {WANTED}; reload the unit to pick it up")
    return NONE


sys.exit(main())
