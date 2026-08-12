#!/usr/bin/env python3
"""Install the isolated Wisent GitHub pre-check runner on a Stado host.

Run this through ``stado host install-helper`` and ``stado host run-helper``.
Before invocation, deliver one short-lived organization runner registration token
as ``github-runner-registration-token`` with ``stado host install-secret``.
The token is consumed through the runner's environment input and deleted before
configuration; it never appears in a remote command line.

The runner executes as the unprivileged ``stado-precheck`` account. Its package,
configuration, and cleanup hooks are root-owned and read-only to jobs. Only the
work and diagnostic directories are writable. A dedicated nftables output chain
blocks that UID from loopback, link-local, RFC1918, and Tailscale/CGNAT networks,
while retaining DNS and public GitHub/package-registry access.
"""

from __future__ import annotations

import grp
import hashlib
import os
import pathlib
import pwd
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request

RUNNER_VERSION = "2.336.0"
RUNNER_SHA256 = "04cf0be1aff4c3ec3554466c39124ca250e3effd8873bb7e8d68535aa9505d5d"
RUNNER_URL = (
    "https://github.com/actions/runner/releases/download/"
    f"v{RUNNER_VERSION}/actions-runner-linux-x64-{RUNNER_VERSION}.tar.gz"
)
RUNNER_USER = "stado-precheck"
RUNNER_GROUP = "stado-precheck"
RUNNER_LABELS = "stado-precheck"
RUNNER_NAME = "stado-precheck-rtx"
RUNNER_ROOT = pathlib.Path("/opt/wisent/stado-precheck-runner")
WORK_DIR = RUNNER_ROOT / "_work"
DIAG_DIR = RUNNER_ROOT / "_diag"
OWNER_NAME = os.environ.get("SUDO_USER") or os.environ.get("USER") or "root"
OWNER_HOME = pathlib.Path(pwd.getpwnam(OWNER_NAME).pw_dir)
TOKEN_FILE = OWNER_HOME / ".stado" / "github-runner-registration-token"
UNIT = pathlib.Path("/etc/systemd/system/wisent-stado-precheck-runner.service")
NFTABLE = "stado_precheck"


def run(*arguments: str, check: bool = True, env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(arguments, text=True, capture_output=True, check=False, env=env)
    if check and result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()
        raise SystemExit(f"{arguments[0]} failed: {detail}")
    return result


def require_root() -> None:
    if os.geteuid() == 0:
        return
    sudo = shutil.which("sudo")
    if not sudo:
        raise SystemExit("root access is required to install the runner")
    os.execv(sudo, [sudo, "-n", sys.executable, str(pathlib.Path(__file__).resolve())])


def runner_identity() -> tuple[int, int]:
    try:
        grp.getgrnam(RUNNER_GROUP)
    except KeyError:
        run("/usr/sbin/groupadd", "--system", RUNNER_GROUP)
    try:
        account = pwd.getpwnam(RUNNER_USER)
    except KeyError:
        run(
            "/usr/sbin/useradd",
            "--system",
            "--gid",
            RUNNER_GROUP,
            "--home-dir",
            str(RUNNER_ROOT),
            "--no-create-home",
            "--shell",
            "/usr/sbin/nologin",
            RUNNER_USER,
        )
        account = pwd.getpwnam(RUNNER_USER)
    if account.pw_uid == 0:
        raise SystemExit("the pre-check runner account must not be root")
    privileged = {"sudo", "wheel", "admin"}
    memberships = {
        group.gr_name
        for group in grp.getgrall()
        if RUNNER_USER in group.gr_mem or group.gr_gid == account.pw_gid
    }
    overlap = sorted(privileged & memberships)
    if overlap:
        raise SystemExit(f"the pre-check runner account is privileged through: {', '.join(overlap)}")
    return account.pw_uid, account.pw_gid


def download_runner(destination: pathlib.Path) -> None:
    digest = hashlib.sha256()
    with urllib.request.urlopen(RUNNER_URL, timeout=120) as response, destination.open("wb") as output:
        while chunk := response.read(1024 * 1024):
            digest.update(chunk)
            output.write(chunk)
    actual = digest.hexdigest()
    if actual != RUNNER_SHA256:
        destination.unlink(missing_ok=True)
        raise SystemExit(f"runner archive checksum mismatch: {actual}")


def safe_extract(archive: pathlib.Path, destination: pathlib.Path) -> None:
    destination_resolved = destination.resolve()
    with tarfile.open(archive, "r:gz") as bundle:
        for member in bundle.getmembers():
            target = (destination / member.name).resolve()
            if destination_resolved not in target.parents and target != destination_resolved:
                raise SystemExit(f"runner archive escapes destination: {member.name}")
        bundle.extractall(destination)


def install_package(uid: int, gid: int) -> None:
    configured = RUNNER_ROOT / ".runner"
    if configured.exists():
        return
    if RUNNER_ROOT.exists():
        shutil.rmtree(RUNNER_ROOT)
    RUNNER_ROOT.mkdir(parents=True, mode=0o755)
    with tempfile.TemporaryDirectory(prefix="stado-precheck-runner-") as temporary:
        archive = pathlib.Path(temporary) / "runner.tar.gz"
        download_runner(archive)
        safe_extract(archive, RUNNER_ROOT)
    for root, directories, files in os.walk(RUNNER_ROOT):
        os.chown(root, uid, gid)
        for name in directories:
            os.chown(os.path.join(root, name), uid, gid)
        for name in files:
            os.chown(os.path.join(root, name), uid, gid)
    WORK_DIR.mkdir(mode=0o700)
    DIAG_DIR.mkdir(mode=0o700)
    os.chown(WORK_DIR, uid, gid)
    os.chown(DIAG_DIR, uid, gid)


def consume_registration_token() -> str:
    if not TOKEN_FILE.is_file():
        raise SystemExit(f"missing short-lived registration token at {TOKEN_FILE}")
    mode = TOKEN_FILE.stat().st_mode & 0o777
    if mode & 0o077:
        raise SystemExit(f"registration token file is too permissive: {mode:o}")
    token = TOKEN_FILE.read_text(encoding="utf-8").strip()
    TOKEN_FILE.unlink()
    if not token:
        raise SystemExit("registration token file was empty")
    return token


def configure_runner(uid: int, gid: int) -> None:
    if (RUNNER_ROOT / ".runner").exists():
        return
    token = consume_registration_token()
    environment = {
        "HOME": pwd.getpwuid(uid).pw_dir,
        "PATH": "/usr/local/bin:/usr/bin:/bin",
        "ACTIONS_RUNNER_INPUT_URL": "https://github.com/wisent-ai",
        "ACTIONS_RUNNER_INPUT_TOKEN": token,
        "ACTIONS_RUNNER_INPUT_NAME": RUNNER_NAME,
        "ACTIONS_RUNNER_INPUT_RUNNERGROUP": RUNNER_GROUP,
        "ACTIONS_RUNNER_INPUT_LABELS": RUNNER_LABELS,
        "ACTIONS_RUNNER_INPUT_WORK": "_work",
    }
    result = run(
        "/usr/sbin/runuser",
        "--user",
        RUNNER_USER,
        "--",
        str(RUNNER_ROOT / "config.sh"),
        "--unattended",
        "--replace",
        "--disableupdate",
        check=False,
        env=environment,
    )
    token = ""
    environment.pop("ACTIONS_RUNNER_INPUT_TOKEN", None)
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()
        raise SystemExit(f"runner configuration failed: {detail}")
    # Jobs must not be able to replace the runner binary, configuration, or
    # lifecycle hooks. The service retains write access only to its work and
    # diagnostic directories through systemd.
    for root, directories, files in os.walk(RUNNER_ROOT):
        os.chown(root, 0, 0)
        os.chmod(root, 0o755)
        for name in directories:
            os.chown(os.path.join(root, name), 0, 0)
        for name in files:
            file_path = os.path.join(root, name)
            os.chown(file_path, 0, 0)
            os.chmod(file_path, 0o755 if os.access(file_path, os.X_OK) else 0o644)
    for writable in (WORK_DIR, DIAG_DIR):
        os.chown(writable, uid, gid)
        os.chmod(writable, 0o700)


def install_cleanup_hook(uid: int, gid: int) -> pathlib.Path:
    hook = RUNNER_ROOT / "clean-work.sh"
    hook.write_text(
        "#!/bin/sh\n"
        "set -eu\n"
        f"work={WORK_DIR!s}\n"
        'find "$work" -mindepth 1 -maxdepth 1 -exec rm -rf -- {} +\n',
        encoding="utf-8",
    )
    os.chown(hook, 0, 0)
    os.chmod(hook, 0o755)
    os.chown(WORK_DIR, uid, gid)
    os.chown(DIAG_DIR, uid, gid)
    return hook


def install_network_boundary(uid: int) -> None:
    nft = shutil.which("nft")
    if not nft:
        raise SystemExit("nftables is required for the pre-check network boundary")
    run(nft, "delete", "table", "inet", NFTABLE, check=False)
    rules = f"""table inet {NFTABLE} {{
  chain output {{
    type filter hook output priority filter; policy accept;
    meta skuid {uid} ip daddr 127.0.0.53 udp dport 53 accept
    meta skuid {uid} ip daddr 127.0.0.53 tcp dport 53 accept
    meta skuid {uid} ip daddr {{ 10.0.0.0/8, 100.64.0.0/10, 127.0.0.0/8, 169.254.0.0/16, 172.16.0.0/12, 192.168.0.0/16 }} reject
    meta skuid {uid} ip6 daddr {{ ::1/128, fc00::/7, fe80::/10 }} reject
  }}
}}
"""
    result = subprocess.run([nft, "--file", "-"], input=rules, text=True, capture_output=True, check=False)
    if result.returncode != 0:
        raise SystemExit(f"nftables boundary failed: {(result.stderr or result.stdout).strip()}")
    persistent = pathlib.Path(f"/etc/nftables.d/{NFTABLE}.nft")
    persistent.parent.mkdir(parents=True, exist_ok=True)
    persistent.write_text(rules, encoding="utf-8")
    os.chmod(persistent, 0o644)
    main_config = pathlib.Path("/etc/nftables.conf")
    include = f'include "{persistent}"'
    current = main_config.read_text(encoding="utf-8") if main_config.exists() else "#!/usr/sbin/nft -f\n\n"
    if include not in current:
        with main_config.open("w", encoding="utf-8") as output:
            output.write(current)
            if current and not current.endswith("\n"):
                output.write("\n")
            output.write(include + "\n")
    os.chmod(main_config, 0o644)
    run("/usr/bin/systemctl", "enable", "nftables.service")


def install_service(hook: pathlib.Path) -> None:
    unit = f"""[Unit]
Description=Wisent isolated GitHub pre-check runner
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User={RUNNER_USER}
Group={RUNNER_USER}
WorkingDirectory={RUNNER_ROOT}
ExecStartPre={hook}
ExecStart={RUNNER_ROOT / 'run.sh'}
Restart=always
RestartSec=5
Environment=ACTIONS_RUNNER_HOOK_JOB_STARTED={hook}
Environment=ACTIONS_RUNNER_HOOK_JOB_COMPLETED={hook}
NoNewPrivileges=true
PrivateTmp=true
PrivateDevices=true
ProtectSystem=strict
ProtectHome=read-only
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectKernelLogs=true
ProtectControlGroups=true
ProtectClock=true
RestrictSUIDSGID=true
LockPersonality=true
ReadWritePaths={WORK_DIR} {DIAG_DIR}

[Install]
WantedBy=multi-user.target
"""
    UNIT.write_text(unit, encoding="utf-8")
    os.chown(UNIT, 0, 0)
    os.chmod(UNIT, 0o644)
    run("/usr/bin/systemctl", "daemon-reload")
    run("/usr/bin/systemctl", "enable", "--now", UNIT.name)


def main() -> int:
    require_root()
    if sys.platform != "linux" or os.uname().machine != "x86_64":
        raise SystemExit("this installer is pinned to the Linux x64 fleet target")
    uid, gid = runner_identity()
    install_package(uid, gid)
    configure_runner(uid, gid)
    hook = install_cleanup_hook(uid, gid)
    install_network_boundary(uid)
    install_service(hook)
    status = run("/usr/bin/systemctl", "is-active", UNIT.name).stdout.strip()
    print(f"runner service: {status}")
    print(f"runner identity: {RUNNER_USER} uid={uid}")
    print(f"runner labels: self-hosted,linux,x64,{RUNNER_LABELS}")
    print("private-network egress: blocked")
    return 0


if __name__ == "__main__":
    sys.exit(main())
