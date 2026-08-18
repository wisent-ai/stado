#!/usr/bin/env python3
"""Report the object API process on THIS host: who listens, what launched it,
and whether its namespace policy already carries the enrollments/ prefix.

Read-only. Exists because the fleet's object API on the always-on host turned
out to be live and UNDECLARED: `stado host inventory` shows the stado-api and
stado-object markers matched on 127.0.0.1:8765 while no service in the registry
directory holds them. Restarting an undeclared process without a model of what
launched it is how the Weles worker died on this same host, so the model comes
first and this script is the instrument that builds it. Runs as a Stado helper
(`stado host install-helper` + `run-helper`), which passes no arguments —
everything here is fixed.
"""

import json
import os
import pathlib
import plistlib
import subprocess

NONE = None
HOME = pathlib.Path(os.path.expanduser("~"))
PORT = "8765"
CONFIG = HOME / ".config" / "stado" / "config.json"
DAEMON_DIRS = (
    pathlib.Path("/Library/LaunchDaemons"),
    HOME / "Library" / "LaunchAgents",
)


def run(*args):
    proc = subprocess.run(args, capture_output=True, text=True, check=False)
    return proc.stdout


def main():
    listener = run("/usr/sbin/lsof", "-nP", f"-iTCP:{PORT}", "-sTCP:LISTEN")
    print("== listener on :%s" % PORT)
    print(listener.strip() or "(nothing listens)")
    pids = [line.split()[1] for line in listener.splitlines()[1:] if line.strip()]
    for pid in dict.fromkeys(pids):
        print(f"== process {pid}")
        print(run("/bin/ps", "-p", pid, "-o", "pid,ppid,user,etime,command").strip())

    print("== launchd plists mentioning stado")
    for directory in DAEMON_DIRS:
        if not directory.is_dir():
            continue
        for plist in sorted(directory.glob("*stado*")):
            print(f"-- {plist}")
            try:
                with plist.open("rb") as handle:
                    document = plistlib.load(handle)
            except Exception as exc:  # unreadable is a fact worth printing
                print(f"   unreadable: {exc}")
                continue
            label = document.get("Label", "?")
            arguments = document.get("ProgramArguments", [])
            keep_alive = document.get("KeepAlive")
            print(f"   Label: {label}")
            print(f"   ProgramArguments: {' '.join(str(a) for a in arguments)}")
            print(f"   KeepAlive: {keep_alive!r}")
            environment = document.get("EnvironmentVariables") or {}
            shown = {
                key: value
                for key, value in environment.items()
                if "TOKEN" not in key and "SECRET" not in key and "UNLOCK" not in key
            }
            print(f"   EnvironmentVariables (nonsecret keys): {shown}")

    print("== namespace policy in %s" % CONFIG)
    if not CONFIG.is_file():
        print("(no config file)")
        return NONE
    document = json.loads(CONFIG.read_text())
    policy = (
        document.get("object_api", {}).get("namespaces", {}).get("probierz", NONE)
    )
    if policy is NONE:
        print("probierz: NOT DECLARED here")
        return NONE
    prefixes = policy.get("prefixes", [])
    print(f"probierz item: {policy.get('item')!r}")
    print(f"probierz prefixes: {len(prefixes)}")
    print(f"has enrollments/: {'enrollments/' in prefixes}")
    return NONE


main()
