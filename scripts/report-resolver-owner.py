#!/usr/bin/env python3
"""Say whether launchd owns the resolver serving this host's adapters.

A resolver started by hand detaches to pid 1 exactly like a daemon does, so
`ps` cannot tell the two apart, and "it survives a restart" is precisely the
claim `ps` cannot support. launchd's own record can: it names the pid it
supervises. This prints that pid beside the pid holding the resolver's API
port, and says plainly whether they are the same process.
"""

import re
import subprocess
import sys

LABEL = "com.wisent.stado-resolver"
API_PORT = "17600"
NONE = None


def run(*args):
    proc = subprocess.run(args, capture_output=True, text=True, check=False)
    return proc.stdout + proc.stderr


def job_state():
    """launchd's own view: the state word, the pid, and how often it has run.

    Reading only `pid` was wrong twice tonight. A job that is between spawns
    reports no pid while a live process holds the port, and calling that
    "hand-started" sent two repairs at the wrong target -- the real fault was a
    second unit competing for the same port.
    """
    text = run("/usr/bin/sudo", "-n", "/bin/launchctl", "print", f"system/{LABEL}")
    if not text.strip().startswith(f"system/{LABEL}"):
        return NONE, NONE, NONE
    def field(name, pattern=r"(.+)"):
        found = re.search(rf"^\s*{name} = {pattern}$", text, re.MULTILINE)
        return found.group(len(["value"])).strip() if found else NONE

    return field("state"), field("pid", r"(\d+)"), field("runs", r"(\d+)")


def listening_pid():
    text = run("/usr/sbin/lsof", "-nP", f"-iTCP:{API_PORT}", "-sTCP:LISTEN")
    for line in text.splitlines()[len(["header"]):]:
        fields = line.split()
        if len(fields) > len(["command"]):
            return fields[len(["command"])]
    return NONE


def main():
    state, supervised, runs = job_state()
    listening = listening_pid()
    print(f"unit {LABEL}: state {state or '(not loaded)'}, pid {supervised or '(none)'}, runs {runs or '?'}")
    print(f"port {API_PORT} held by {listening or '(nobody)'}")
    if supervised and supervised == listening:
        print("verdict managed: this resolver returns after a restart")
    elif listening and state:
        print(
            "verdict contended: the unit is loaded but the port belongs to another "
            "process; look for a second unit or an orphan"
        )
    elif listening:
        print("verdict unmanaged: this resolver is a hand-started process")
    else:
        print("verdict down: nothing is serving the resolver API")
    return NONE


sys.exit(main())
