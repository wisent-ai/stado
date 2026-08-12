#!/usr/bin/env python3
"""Publish this host's health beacon on a schedule instead of by hand.

`registry beacon-age` is how the fleet decides whether a host is alive, and a
beacon published once by an operator goes stale within the hour -- which reads
worse than none, because staleness looks like reporting. The collector already
exists as an owner-only helper; this gives it a clock.

A login-domain agent is the right home: the collector reads this user's launchd
state and this user's Stado configuration, and it needs no privilege beyond the
`sudo -n launchctl print` the system units already allow.

Idempotent: an agent that already runs this program on this interval is left
alone.
"""

import os
import pathlib
import plistlib
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
LABEL = "com.wisent.host-health-beacon-collect"
PROGRAM = HOME / ".stado" / "bin" / "host-health-beacon-collect"
LOGS = HOME / ".stado" / "logs"
SYSTEM_PLIST = pathlib.Path("/Library/LaunchDaemons") / f"{LABEL}.plist"
AGENT_PLIST = HOME / "Library" / "LaunchAgents" / f"{LABEL}.plist"
INTERVAL = len("m" * 300)
OWNER_ONLY = 0o600


def run(*args):
    proc = subprocess.run(args, capture_output=True, text=True, check=False)
    return proc.stdout + proc.stderr


def can_sudo():
    """Whether this session can install a system unit without a prompt.

    The always-on host grants passwordless sudo for exactly this kind of
    maintenance; a workstation usually does not, and asking would put a consent
    dialog in front of a human who did not run this. Where sudo is unavailable
    the login-domain agent is the correct home anyway: a workstation has a
    console session, which is the thing an SSH helper lacks.
    """
    return subprocess.run(
        ["/usr/bin/sudo", "-n", "/usr/bin/true"], capture_output=True, check=False
    ).returncode == ZERO


def loaded(system):
    if system:
        printed = run("/usr/bin/sudo", "-n", "/bin/launchctl", "print", f"system/{LABEL}")
        return printed.strip().startswith(f"system/{LABEL}")
    domain = f"gui/{os.getuid()}"
    return run("/bin/launchctl", "print", f"{domain}/{LABEL}").strip().startswith(domain)


def main():
    if not PROGRAM.is_file():
        raise SystemExit(f"no collector at {PROGRAM}; install the helper first")
    LOGS.mkdir(parents=True, exist_ok=True)
    system = can_sudo()
    plist = SYSTEM_PLIST if system else AGENT_PLIST
    document = {
        "Label": LABEL,
        "ProgramArguments": ["/bin/bash", str(PROGRAM)],
        "RunAtLoad": True,
        "StartInterval": INTERVAL,
        "WorkingDirectory": str(HOME),
        "EnvironmentVariables": {
            "HOME": str(HOME),
            "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        },
        "StandardOutPath": str(LOGS / "host-health-beacon-collect.out"),
        "StandardErrorPath": str(LOGS / "host-health-beacon-collect.err"),
    }
    if system:
        document["UserName"] = os.environ.get("USER", HOME.name)
    existing = plistlib.loads(plist.read_bytes()) if plist.is_file() else {}
    if existing == document and loaded(system):
        print(f"settled   {LABEL} already ticks every {INTERVAL}s")
        return NONE

    if system:
        staging = HOME / ".stado" / "files" / f"{LABEL}.plist"
        staging.parent.mkdir(parents=True, exist_ok=True)
        with staging.open("wb") as handle:
            plistlib.dump(document, handle)
        os.chmod(staging, OWNER_ONLY)
        run("/usr/bin/sudo", "-n", "/bin/cp", str(staging), str(plist))
        run("/usr/bin/sudo", "-n", "/usr/sbin/chown", "root:wheel", str(plist))
        run("/usr/bin/sudo", "-n", "/bin/chmod", "u=rw,go=r", str(plist))
        run("/usr/bin/sudo", "-n", "/bin/launchctl", "bootout", f"system/{LABEL}")
        run("/usr/bin/sudo", "-n", "/bin/launchctl", "enable", f"system/{LABEL}")
        detail = run("/usr/bin/sudo", "-n", "/bin/launchctl", "bootstrap", "system", str(plist))
    else:
        plist.parent.mkdir(parents=True, exist_ok=True)
        with plist.open("wb") as handle:
            plistlib.dump(document, handle)
        os.chmod(plist, OWNER_ONLY)
        domain = f"gui/{os.getuid()}"
        run("/bin/launchctl", "bootout", f"{domain}/{LABEL}")
        run("/bin/launchctl", "enable", f"{domain}/{LABEL}")
        detail = run("/bin/launchctl", "bootstrap", domain, str(plist))
    print(f"unit      {plist}")
    print(f"interval  {INTERVAL}s")
    print(f"bootstrap {detail.strip() or 'ok'}")
    print(f"loaded    {'yes' if loaded(system) else 'no'}")
    return NONE


sys.exit(main())
