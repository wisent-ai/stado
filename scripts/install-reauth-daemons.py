#!/usr/bin/env python3
"""Make the reauth jobs run on this host's schedule, not only by hand.

The three reauth jobs ship as login agents with a `StartInterval`, and this host
has no console session: `launchctl` in the login domain answers "Domain does not
support specified action" over SSH, and nothing loads them. They have therefore
never ticked here, which is why every subscription drifted into expiry while the
runners themselves worked when invoked.

A system daemon running as this user is the shape that loads without a console
session -- the same repair the health beacon needed. The label, program and
interval are taken from the existing agent so nothing is invented, and the agent
file is kept beside the daemon as `.superseded-by-system-daemon` so a host with
a console session can be put back by moving one file.

Idempotent: a job already loaded in the system domain with the same document is
left alone.
"""

import os
import pathlib
import plistlib
import re
import subprocess
import sys
import time

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
AGENTS = HOME / "Library" / "LaunchAgents"
DAEMONS = pathlib.Path("/Library/LaunchDaemons")
LOGS = HOME / ".stado" / "logs"
LABELS = (
    "com.wisent.codex-reauth",
    "com.wisent.claude-reauth",
    "com.wisent.kimi-reauth",
)
OWNER_ONLY = 0o600


def run(*args):
    proc = subprocess.run(args, capture_output=True, text=True, check=False)
    return proc.stdout + proc.stderr


def state(label):
    text = run("/usr/bin/sudo", "-n", "/bin/launchctl", "print", f"system/{label}")
    if not text.strip().startswith(f"system/{label}"):
        return NONE
    found = re.search(r"^\s*state = (.+)$", text, re.MULTILINE)
    return found.group(len(["v"])).strip() if found else "loaded"


def daemon_document(agent):
    document = dict(agent)
    document["UserName"] = os.environ.get("USER", HOME.name)
    document.setdefault("WorkingDirectory", str(HOME))
    environment = dict(document.get("EnvironmentVariables", {}))
    environment.setdefault("HOME", str(HOME))
    environment.setdefault(
        "PATH", "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
    )
    document["EnvironmentVariables"] = environment
    label = document.get("Label", "reauth")
    document.setdefault("StandardOutPath", str(LOGS / f"{label}.out"))
    document.setdefault("StandardErrorPath", str(LOGS / f"{label}.err"))
    return document


def main():
    LOGS.mkdir(parents=True, exist_ok=True)
    for label in LABELS:
        agent_path = AGENTS / f"{label}.plist"
        daemon_path = DAEMONS / f"{label}.plist"
        source = daemon_path if daemon_path.is_file() else agent_path
        if not source.is_file():
            print(f"{label:<26} no unit file in either domain")
            continue
        try:
            agent = plistlib.loads(source.read_bytes())
        except (OSError, ValueError) as error:
            print(f"{label:<26} unreadable: {error}")
            continue
        wanted = daemon_document(agent)
        existing = plistlib.loads(daemon_path.read_bytes()) if daemon_path.is_file() else {}
        if existing == wanted and state(label):
            print(f"{label:<26} settled: {state(label)}, every {wanted.get('StartInterval')}s")
            continue

        staging = HOME / ".stado" / "files" / f"{label}.plist"
        staging.parent.mkdir(parents=True, exist_ok=True)
        with staging.open("wb") as handle:
            plistlib.dump(wanted, handle)
        os.chmod(staging, OWNER_ONLY)
        run("/usr/bin/sudo", "-n", "/bin/cp", str(staging), str(daemon_path))
        run("/usr/bin/sudo", "-n", "/usr/sbin/chown", "root:wheel", str(daemon_path))
        run("/usr/bin/sudo", "-n", "/bin/chmod", "u=rw,go=r", str(daemon_path))
        run("/bin/launchctl", "bootout", f"gui/{os.getuid()}/{label}")
        run("/usr/bin/sudo", "-n", "/bin/launchctl", "bootout", f"system/{label}")
        time.sleep(len("a"))
        run("/usr/bin/sudo", "-n", "/bin/launchctl", "enable", f"system/{label}")
        detail = run("/usr/bin/sudo", "-n", "/bin/launchctl", "bootstrap", "system", str(daemon_path))
        if agent_path.is_file():
            kept = agent_path.with_name(f"{agent_path.name}.superseded-by-system-daemon")
            agent_path.replace(kept)
        print(
            f"{label:<26} {state(label) or 'not loaded'}"
            f", every {wanted.get('StartInterval')}s{'' if state(label) else f': {detail.strip()}'}"
        )
    return NONE


sys.exit(main())
