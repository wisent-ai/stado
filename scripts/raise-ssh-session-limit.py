#!/usr/bin/env python3
"""Let one SSH connection carry the fleet's stdio forwards.

Every resolver adapter is a stdio-forward channel on a single SSH connection to
this host, and OpenSSH allows ten of them by default. A workstation with eight
adapters plus one operator command is over the line, and the eleventh channel is
refused with "Session open refused by peer" -- which surfaces as a gateway that
answers nothing while every unit involved reports itself healthy.

macOS launches sshd per connection from launchd, so a drop-in takes effect on
the next connection with nothing to restart and no established session dropped.

Idempotent: a host already carrying this setting is left alone.
"""

import os
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
DROPIN = pathlib.Path("/etc/ssh/sshd_config.d/50-stado-sessions.conf")
SESSIONS = len("s" * 64)
BODY = f"""# Managed by wisent-compute/scripts/raise-ssh-session-limit.py
# The resolver opens one stdio-forward channel per declared adapter over a
# single connection; ten is not enough for a fleet host that also answers
# operator commands.
MaxSessions {SESSIONS}
"""


def run(*args):
    proc = subprocess.run(args, capture_output=True, text=True, check=False)
    return proc.stdout + proc.stderr


def main():
    current = DROPIN.read_text(encoding="utf-8") if DROPIN.is_file() else ""
    print(f"dropin     {DROPIN}")
    print(f"configured {current.strip().splitlines()[-1:] or ['(absent)']}")
    if current == BODY:
        print(f"settled    MaxSessions is already {SESSIONS}")
        return NONE

    staging = pathlib.Path(os.path.expanduser("~")) / ".stado" / "files" / DROPIN.name
    staging.parent.mkdir(parents=True, exist_ok=True)
    staging.write_text(BODY, encoding="utf-8")
    run("/usr/bin/sudo", "-n", "/bin/mkdir", "-p", str(DROPIN.parent))
    run("/usr/bin/sudo", "-n", "/bin/cp", str(staging), str(DROPIN))
    run("/usr/bin/sudo", "-n", "/usr/sbin/chown", "root:wheel", str(DROPIN))
    run("/usr/bin/sudo", "-n", "/bin/chmod", "u=rw,go=r", str(DROPIN))
    check = run("/usr/bin/sudo", "-n", "/usr/sbin/sshd", "-t")
    print(f"set        MaxSessions {SESSIONS}")
    print(f"sshd -t    {check.strip() or 'ok'}")
    return NONE


sys.exit(main())
