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

LABEL = "com.wisent.always-on.stado-resolver"
API_PORT = "17600"
NONE = None


def run(*args):
    proc = subprocess.run(args, capture_output=True, text=True, check=False)
    return proc.stdout + proc.stderr


def supervised_pid():
    text = run("/usr/bin/sudo", "-n", "/bin/launchctl", "print", f"system/{LABEL}")
    found = re.search(r"^\s*pid = (\d+)", text, re.MULTILINE)
    return found.group(len(["pid"])) if found else NONE


def listening_pid():
    text = run("/usr/sbin/lsof", "-nP", f"-iTCP:{API_PORT}", "-sTCP:LISTEN")
    for line in text.splitlines()[len(["header"]):]:
        fields = line.split()
        if len(fields) > len(["command"]):
            return fields[len(["command"])]
    return NONE


def main():
    supervised = supervised_pid()
    listening = listening_pid()
    print(f"launchd supervises {supervised or '(nothing)'}")
    print(f"port {API_PORT} held by {listening or '(nobody)'}")
    if supervised and supervised == listening:
        print("verdict managed: this resolver returns after a restart")
    elif listening:
        print("verdict unmanaged: this resolver is a hand-started process")
    else:
        print("verdict down: nothing is serving the resolver API")
    return NONE


sys.exit(main())
