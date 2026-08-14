#!/usr/bin/env python3
"""Load the tailnet object proxy and prove what it is listening on.

The proxy is the front door three configurations already name
(`https://100.120.25.24:8765`), and it has never run: it was installed as a
LaunchAgent in the `gui/501` domain of a host where nobody logs in, so launchd
answered `Unrecognized target specifier` and every off-host reader saw a refused
connection instead of the store. It now exists as a system daemon, which needs
no session.

Loading it is only half the check. A proxy that binds the wrong interface is a
different defect wearing the same green light, so this prints every listener on
the port and refuses to call the job healthy unless the tailnet address is bound
and the loopback listener -- the object API itself -- is still there.
"""

import os
import pathlib
import subprocess
import sys
import time

NONE = None
ZERO = len([])
LABEL = "com.wisent.always-on.stado-tailnet-object-proxy"
UNIT = pathlib.Path("/Library/LaunchDaemons") / f"{LABEL}.plist"
TAILNET = "100.120.25.24:8765"
LOOPBACK = "127.0.0.1:8765"
SETTLE = 20


def run(*args):
    proc = subprocess.run(args, capture_output=True, text=True, check=False)
    return proc.stdout + proc.stderr


def listeners():
    rows = []
    for line in run("/usr/sbin/lsof", "-nP", "-iTCP:8765", "-sTCP:LISTEN").splitlines()[1:]:
        parts = line.split()
        if len(parts) >= len("abcdefghi"):
            rows.append((parts[ZERO], parts[2], parts[-2]))
    return rows


def state():
    for line in run("/bin/launchctl", "print", f"system/{LABEL}").splitlines():
        stripped = line.strip()
        if stripped.startswith("state = "):
            return stripped[len("state = "):]
    return "not loaded"


def main():
    if not UNIT.is_file():
        raise SystemExit(f"no unit at {UNIT}")
    print(f"before  state {state()}")
    for command, owner, address in listeners():
        print(f"        {command:12} {owner:10} {address}")
    detail = run("/usr/bin/sudo", "-n", "/bin/launchctl", "bootstrap", "system", str(UNIT))
    print(f"load    {detail.strip() or 'accepted'}")
    deadline = time.time() + SETTLE
    while time.time() < deadline:
        if any(address.endswith(TAILNET) for _, _, address in listeners()):
            break
        time.sleep(len("ab"))
    print(f"after   state {state()}")
    bound = listeners()
    for command, owner, address in bound:
        print(f"        {command:12} {owner:10} {address}")
    addresses = [address for _, _, address in bound]
    if any(address.startswith("0.0.0.0") or address.startswith("*") for address in addresses):
        raise SystemExit("something is listening on every interface; stop and look before going further")
    if not any(address.endswith(TAILNET) for address in addresses):
        raise SystemExit("the proxy did not bind the tailnet address; the front door is still shut")
    if not any(address.endswith(LOOPBACK) for address in addresses):
        raise SystemExit("the object API's own loopback listener is gone; that is worse than the problem")
    print("verdict the declared tailnet endpoint is answered by a daemon that needs no session")
    return NONE


sys.exit(main())
