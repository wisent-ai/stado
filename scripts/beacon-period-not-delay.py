#!/usr/bin/env python3
"""Make the beacon loop a period, and put two publishes inside the read window.

The always-on beacon on a control-plane host is a KeepAlive loop, not a launchd
`StartInterval` job: it publishes, then sleeps `WC_HEALTH_BEACON_INTERVAL`. Two
things followed from that, and both showed up as `registry doctor` flapping on
`stale-beacon` for a host that was healthy and publishing:

  * the sleep started after the work, so the real period was run + sleep;
  * the period was 60 s against a 180 s staleness window, so a single lost
    cycle -- and this host's log carries thousands of them -- put the document
    over the line.

So the loop now sleeps what is left of the period (floor {FLOOR} s, so a slow
host cannot spin), and the period is {PERIOD} s: two publishes inside the window
mean one lost cycle is not a stale host. The launcher is a host-installed
artifact, so this is idempotent, keeps a copy beside it, and refuses to write a
launcher that would not parse.
"""

import os
import pathlib
import subprocess
import sys

NONE = None
FLOOR = 15
PERIOD = 30
HOME = pathlib.Path(os.path.expanduser("~"))
LAUNCHER = HOME / ".stado" / "bin" / "host-health-beacon-launcher"
MARKER = "# period, not delay:"
OLD_SLEEP = '/bin/sleep "$INTERVAL"'
NEW_SLEEP = f"""{MARKER} the publish is part of the period, not extra to it,
  # and a reader calls the document stale after 180s.
  elapsed=$(( $(/bin/date +%s) - cycle_started ))
  remaining=$(( INTERVAL - elapsed ))
  [ "$remaining" -lt {FLOOR} ] && remaining={FLOOR}
  /bin/sleep "$remaining\""""


def pin_period(body):
    """Two publishes inside the window, so one lost cycle is not a stale host."""
    wanted = f'INTERVAL="${{WC_HEALTH_BEACON_INTERVAL:-{PERIOD}}}"'
    if wanted in body:
        return body, f"already {PERIOD}s"
    for old in (60, 120, 300):
        candidate = f'INTERVAL="${{WC_HEALTH_BEACON_INTERVAL:-{old}}}"'
        if candidate in body:
            return body.replace(candidate, wanted, 1), f"{old}s -> {PERIOD}s"
    return body, "not declared inline; left alone"


def stamp_loop(body):
    """Sleep the remainder of the period rather than a fresh delay after work."""
    if MARKER in body:
        return body, "already a period"
    if OLD_SLEEP not in body:
        raise SystemExit(f"{LAUNCHER} does not sleep with {OLD_SLEEP}; not editing blind")
    stamped = body.replace(OLD_SLEEP, NEW_SLEEP, 1)
    if "cycle_started=" not in stamped:
        opener = "while :; do"
        if opener not in stamped:
            raise SystemExit(f"{LAUNCHER} has no `{opener}` loop to stamp")
        stamped = stamped.replace(opener, f"{opener}\n  cycle_started=$(/bin/date +%s)", 1)
    return stamped, "sleeps INTERVAL minus the time the publish took"


def main():
    if not LAUNCHER.is_file():
        raise SystemExit(f"no beacon launcher at {LAUNCHER}")
    original = LAUNCHER.read_text(encoding="utf-8")
    print(f"launcher   {LAUNCHER}")
    body, cadence = pin_period(original)
    body, loop = stamp_loop(body)
    print(f"period     {cadence}")
    print(f"loop       {loop}")
    if body == original:
        print("settled    nothing to change")
        return NONE
    LAUNCHER.with_name(LAUNCHER.name + ".before-period").write_text(original, encoding="utf-8")
    LAUNCHER.write_text(body, encoding="utf-8")
    check = subprocess.run(["/bin/sh", "-n", str(LAUNCHER)], capture_output=True,
                           text=True, check=False)
    if check.returncode != 0:
        LAUNCHER.write_text(original, encoding="utf-8")
        raise SystemExit(f"edit refused, launcher restored: {check.stderr.strip()}")
    print("written    launcher parses; reload the unit to pick it up")
    return NONE


sys.exit(main())
