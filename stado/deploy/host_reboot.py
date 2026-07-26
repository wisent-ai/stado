"""Registry-authorized reboot of a managed macOS host via the approved channel.

Mirrors host_recovery's trust model: the registry selects only the host, the
remote program is fixed (a graceful shutdown), and BatchMode ssh is the only
transport. No shell fragments come from registry data.
"""
from __future__ import annotations

import subprocess
from typing import Any

from .host_recovery import _target


def reboot_host(target_name: str) -> dict[str, Any]:
    """Request a graceful reboot on one canonical registry host."""
    target = _target(target_name)
    process = subprocess.run(
        [
            "ssh",
            "-o", "BatchMode=yes",
            "-o", "StrictHostKeyChecking=accept-new",
            "-o", "ConnectTimeout=15",
            target.ssh,
            "sudo -n /sbin/shutdown -r now",
        ],
        capture_output=True,
        text=True,
        check=False,
    )
    stderr = (process.stderr or "").strip()
    ok = not process.returncode
    report: dict[str, Any] = {
        "target": target.name,
        "ssh": target.ssh,
        "exit_code": process.returncode,
        "status": "reboot_requested" if ok else "failed",
    }
    if not ok:
        # The common failure is sudo requiring a password; surface it verbatim
        # so the operator knows whether to grant passwordless shutdown or
        # reboot the box physically.
        lines = stderr.splitlines()
        report["error"] = next(reversed(lines), "") if lines else "ssh failed"
    return report
