#!/usr/bin/env python3
"""Place the tailnet object proxy in the system domain, without loading it.

The proxy was a LaunchAgent in a user domain the always-on host does not have,
so it never ran and the declared `https://<tailnet>:8765` endpoint was refused
for every off-host caller. The unit definition beside this script
(`deploy/com.wisent.always-on.stado-tailnet-object-proxy.plist`) is the same
program with the same launcher, TLS material and bind address, in the domain a
machine with no console session actually has.

Delivery, not activation: this installs the plist, retires the agent definition
that can never load, and stops. Loading a daemon that fronts the fleet's store
is an operator action with an owner watching the fleet, and it is one command:

    sudo launchctl bootstrap system \\
        /Library/LaunchDaemons/com.wisent.always-on.stado-tailnet-object-proxy.plist

Idempotent: it compares digests and rewrites only on a difference. It prints
what it found, what it wrote, and whether the endpoint answers, and it never
prints the contents of a credential.
"""

import hashlib
import json
import os
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
LABEL = "com.wisent.always-on.stado-tailnet-object-proxy"
DAEMON = pathlib.Path("/Library/LaunchDaemons") / f"{LABEL}.plist"
AGENT = HOME / "Library" / "LaunchAgents" / "com.wisent.compute.service.stado-tailnet-object-proxy.plist"
# `stado host install-file` lands operator payloads here, checksummed on arrival,
# which is the only way a file of this repo's reaches a fleet host.
DELIVERED = HOME / ".stado" / "files" / f"{LABEL}.plist"
LAUNCHER = HOME / ".stado" / "bin" / "start-stado-tailnet-object-proxy"
# The launcher serves ~/.stado/stado-tailnet-server.crt with its matching key. The
# authority behind them is Skarbiec item `stado-tailnet-ca` in this host's vault:
# re-issue the leaf from that item with scripts/mint-tailnet-authority.py, swap it
# with scripts/swap-tailnet-server-certificate.py, and reload with
# scripts/reload-tailnet-object-proxy.py, which does not need root because this
# unit declares UserName and KeepAlive.
LOGS = HOME / ".stado" / "logs"


def run(*args):
    proc = subprocess.run(
        args, capture_output=True, text=True, check=False, timeout=len("a" * 60)
    )
    return proc.returncode, (proc.stdout + proc.stderr).strip()


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()[: len("a" * 12)] if path.is_file() else "-"


def main():
    if not DELIVERED.is_file():
        raise SystemExit(
            f"{DELIVERED} is absent; deliver it first with `stado host install-file "
            f"<host> deploy/{LABEL}.plist {LABEL}.plist`"
        )
    if not LAUNCHER.is_file():
        raise SystemExit(f"{LAUNCHER} is absent; this host does not carry the proxy launcher")
    body = DELIVERED.read_text(encoding="utf-8")
    # The unit must name the launcher this host actually has, or it would install
    # a daemon that fails on first start in a way only the log would show.
    if str(LAUNCHER) not in body:
        raise SystemExit(f"the delivered unit does not run {LAUNCHER}; refusing to install it")
    print(f"launcher    {LAUNCHER}")
    for line in LAUNCHER.read_text("utf-8", "replace").splitlines():
        if line.strip().startswith("export STADO_"):
            print(f"  {line.strip()}")
    print(f"delivered   {DELIVERED}  sha256 {digest(DELIVERED)}")
    print(f"installed   {DAEMON}  sha256 {digest(DAEMON)}")

    if digest(DAEMON) != digest(DELIVERED):
        code, output = run("/usr/bin/sudo", "-n", "/bin/cp", str(DELIVERED), str(DAEMON))
        if code != ZERO:
            raise SystemExit(f"could not write {DAEMON}: {output}")
        for command in (
            ("/usr/sbin/chown", "root:wheel", str(DAEMON)),
            ("/bin/chmod", "644", str(DAEMON)),
        ):
            code, output = run("/usr/bin/sudo", "-n", *command)
            if code != ZERO:
                raise SystemExit(f"could not {command[ZERO]} {DAEMON}: {output}")
        print(f"wrote       {DAEMON}  sha256 {digest(DAEMON)}")
    else:
        print("settled     the system daemon already matches the delivered unit")

    LOGS.mkdir(parents=True, exist_ok=True)
    # The agent definition is not merely inactive, it is unloadable on this host,
    # and leaving it in place is how a machine ends up with two declarations of
    # one service and no way to tell which one is meant to run.
    if AGENT.is_file():
        retired = AGENT.with_suffix(".plist.retired-into-system-domain")
        AGENT.replace(retired)
        print(f"retired     {AGENT} -> {retired.name}")
    else:
        print("retired     (no user-domain agent definition remains)")

    code, printed = run("/usr/bin/sudo", "-n", "/bin/launchctl", "print", f"system/{LABEL}")
    state = [line.strip() for line in printed.splitlines() if line.strip().startswith("state")]
    print(f"launchd     {state[ZERO] if state else 'not loaded'}")
    print(f"label       system/{LABEL}")
    print("load with   sudo launchctl bootstrap system " + str(DAEMON))
    return NONE


sys.exit(main())
