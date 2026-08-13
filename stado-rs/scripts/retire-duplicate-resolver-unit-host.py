#!/usr/bin/env python3
"""Retire the superseded ubuntu-owned resolver before starting the managed unit."""

from pathlib import Path
import os
import pwd
import subprocess

OLD_UNIT = "stado-service-resolver.service"
OLD_PATH = Path("/home/ubuntu/.config/systemd/user") / OLD_UNIT
MANAGED_PATH = Path(
    "/root/.config/systemd/user/com.wisent.compute.service.stado-resolver.service"
)

if os.geteuid() != 0:
    raise SystemExit("resolver unit reconciliation must run as root")
if not MANAGED_PATH.is_file():
    raise SystemExit(f"managed resolver unit is missing: {MANAGED_PATH}")
if not OLD_PATH.exists():
    print(f"superseded resolver unit is already absent: {OLD_PATH}")
    raise SystemExit(0)

ubuntu = pwd.getpwnam("ubuntu")
user_runtime = f"/run/user/{ubuntu.pw_uid}"
user_environment = [
    f"XDG_RUNTIME_DIR={user_runtime}",
    f"DBUS_SESSION_BUS_ADDRESS=unix:path={user_runtime}/bus",
]
command = [
    "/usr/sbin/runuser",
    "-u",
    "ubuntu",
    "--",
    "/usr/bin/env",
    *user_environment,
    "/usr/bin/systemctl",
    "--user",
]
subprocess.run([*command, "disable", "--now", OLD_UNIT], check=True)
OLD_PATH.unlink()
subprocess.run([*command, "daemon-reload"], check=True)
print(f"retired superseded resolver unit: {OLD_PATH}")
print(f"managed resolver unit retained: {MANAGED_PATH}")
