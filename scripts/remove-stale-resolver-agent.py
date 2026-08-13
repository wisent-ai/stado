#!/usr/bin/env python3
"""Keep one unit file per label on an always-on host.

The resolver is declared twice here: a system daemon that actually runs, and a
login agent left in `~/Library/LaunchAgents` with the same label. The agent
cannot load from an SSH session, so it does nothing most of the time -- but a
console login would start it, and two processes would then fight for the same
loopback ports. It also shadows the daemon during discovery: `service adopt`
probes the login domain first and recorded the agent's path as the unit's
location, which is a registry that describes a file nobody runs.

Removed only when the system daemon exists and is loaded, so a host whose only
copy is the agent keeps it.
"""

import os
import pathlib
import re
import subprocess
import sys

NONE = None
LABEL = "com.wisent.stado-resolver"
HOME = pathlib.Path(os.path.expanduser("~"))
AGENT = HOME / "Library" / "LaunchAgents" / f"{LABEL}.plist"
DAEMON = pathlib.Path("/Library/LaunchDaemons") / f"{LABEL}.plist"


def run(*args):
    proc = subprocess.run(args, capture_output=True, text=True, check=False)
    return proc.stdout + proc.stderr


def daemon_running():
    text = run("/usr/bin/sudo", "-n", "/bin/launchctl", "print", f"system/{LABEL}")
    return bool(re.search(r"^\s*state = running$", text, re.MULTILINE))


def main():
    print(f"daemon     {DAEMON} {'present' if DAEMON.is_file() else 'absent'}")
    print(f"agent      {AGENT} {'present' if AGENT.is_file() else 'absent'}")
    if not AGENT.is_file():
        print("settled    only one unit file carries this label")
        return NONE
    if not (DAEMON.is_file() and daemon_running()):
        print("refusing   the system daemon is not running; the agent is the only copy left")
        return len("x")
    run("/bin/launchctl", "bootout", f"gui/{os.getuid()}/{LABEL}")
    keep = AGENT.with_name(f"{AGENT.name}.superseded-by-system-daemon")
    AGENT.replace(keep)
    print(f"moved      {AGENT} -> {keep}")
    print(f"running    system/{LABEL} state running")
    return NONE


sys.exit(main())
