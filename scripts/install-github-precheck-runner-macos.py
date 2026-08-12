#!/usr/bin/env python3
"""Install the isolated Wisent GitHub pre-check runner on the Stado Mac host.

Run through ``stado host install-helper`` and ``stado host run-helper`` after a
short-lived organization runner registration token has been delivered as
``github-runner-registration-token`` with ``stado host install-secret``. The
helper creates a hidden, password-disabled local account, pins and verifies the
runner release, installs per-job cleanup hooks, and loads a PF anchor that blocks
that account from loopback, link-local, private, and Tailscale/CGNAT networks.
"""

from __future__ import annotations

import grp
import hashlib
import os
import pathlib
import plistlib
import pwd
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request

RUNNER_VERSION = "2.336.0"
RUNNER_SHA256 = "8e8839c49b7060b6b2154f4931f815df330c27f167d53ef2239ee3dfce28b079"
RUNNER_URL = (
    "https://github.com/actions/runner/releases/download/"
    f"v{RUNNER_VERSION}/actions-runner-osx-arm64-{RUNNER_VERSION}.tar.gz"
)
RUNNER_USER = "stado-precheck"
RUNNER_GROUP = "stado-precheck"
RUNNER_LABELS = "stado-precheck"
RUNNER_NAME = "stado-precheck-mac-mini"
RUNNER_ROOT = pathlib.Path("/Users/Shared/stado-precheck-runner")
WORK_DIR = RUNNER_ROOT / "_work"
DIAG_DIR = RUNNER_ROOT / "_diag"
OWNER_NAME = os.environ.get("SUDO_USER") or os.environ.get("USER") or "root"
OWNER_HOME = pathlib.Path(pwd.getpwnam(OWNER_NAME).pw_dir)
TOKEN_FILE = OWNER_HOME / ".stado" / "github-runner-registration-token"
LABEL = "com.wisent.stado-precheck-runner"
PLIST = pathlib.Path(f"/Library/LaunchDaemons/{LABEL}.plist")
PF_ANCHOR_NAME = "com.wisent.stado-precheck"
PF_ANCHOR = pathlib.Path(f"/etc/pf.anchors/{PF_ANCHOR_NAME}")


def run(*arguments: str, check: bool = True, env: dict[str, str] | None = None, **kwargs) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(arguments, text=True, capture_output=True, check=False, env=env, **kwargs)
    if check and result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()
        raise SystemExit(f"{arguments[0]} failed: {detail}")
    return result


def require_root() -> None:
    if os.geteuid() == 0:
        return
    os.execv(
        "/usr/bin/sudo",
        ["/usr/bin/sudo", "-n", sys.executable, str(pathlib.Path(__file__).resolve())],
    )


def free_local_id() -> int:
    occupied = {entry.pw_uid for entry in pwd.getpwall()} | {entry.gr_gid for entry in grp.getgrall()}
    for identifier in range(450, 500):
        if identifier not in occupied:
            return identifier
    raise SystemExit("no free local service-account id in the 450-499 range")


def runner_identity() -> tuple[int, int]:
    try:
        group = grp.getgrnam(RUNNER_GROUP)
    except KeyError:
        identifier = free_local_id()
        run("/usr/bin/dscl", ".", "-create", f"/Groups/{RUNNER_GROUP}")
        run("/usr/bin/dscl", ".", "-create", f"/Groups/{RUNNER_GROUP}", "PrimaryGroupID", str(identifier))
        run("/usr/bin/dscl", ".", "-create", f"/Groups/{RUNNER_GROUP}", "RealName", "Wisent precheck runner")
        group = grp.getgrnam(RUNNER_GROUP)
    try:
        account = pwd.getpwnam(RUNNER_USER)
    except KeyError:
        run("/usr/bin/dscl", ".", "-create", f"/Users/{RUNNER_USER}")
        run("/usr/bin/dscl", ".", "-create", f"/Users/{RUNNER_USER}", "UniqueID", str(free_local_id()))
        run("/usr/bin/dscl", ".", "-create", f"/Users/{RUNNER_USER}", "PrimaryGroupID", str(group.gr_gid))
        run("/usr/bin/dscl", ".", "-create", f"/Users/{RUNNER_USER}", "NFSHomeDirectory", str(RUNNER_ROOT))
        run("/usr/bin/dscl", ".", "-create", f"/Users/{RUNNER_USER}", "UserShell", "/bin/sh")
        run("/usr/bin/dscl", ".", "-create", f"/Users/{RUNNER_USER}", "IsHidden", "1")
        run("/usr/bin/dscl", ".", "-passwd", f"/Users/{RUNNER_USER}", "*")
        account = pwd.getpwnam(RUNNER_USER)
    if account.pw_uid == 0 or account.pw_gid in {0, 80}:
        raise SystemExit("the pre-check runner account must not be an administrator")
    administrators = run("/usr/sbin/dseditgroup", "-o", "checkmember", "-m", RUNNER_USER, "admin", check=False)
    if "yes" in administrators.stdout.lower():
        raise SystemExit("the pre-check runner account belongs to the admin group")
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


def chown_tree(path: pathlib.Path, uid: int, gid: int) -> None:
    for root, directories, files in os.walk(path):
        os.chown(root, uid, gid)
        for name in directories:
            os.chown(os.path.join(root, name), uid, gid)
        for name in files:
            os.chown(os.path.join(root, name), uid, gid)


def install_package(uid: int, gid: int) -> None:
    if (RUNNER_ROOT / ".runner").exists():
        return
    if RUNNER_ROOT.exists():
        shutil.rmtree(RUNNER_ROOT)
    RUNNER_ROOT.mkdir(parents=True, mode=0o755)
    with tempfile.TemporaryDirectory(prefix="stado-precheck-runner-") as temporary:
        archive = pathlib.Path(temporary) / "runner.tar.gz"
        download_runner(archive)
        safe_extract(archive, RUNNER_ROOT)
    chown_tree(RUNNER_ROOT, uid, gid)
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


def drop_privileges(uid: int, gid: int):
    def demote() -> None:
        os.setgroups([])
        os.setgid(gid)
        os.setuid(uid)
    return demote


def configure_runner(uid: int, gid: int) -> None:
    if (RUNNER_ROOT / ".runner").exists():
        return
    token = consume_registration_token()
    environment = {
        "HOME": str(RUNNER_ROOT),
        "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        "ACTIONS_RUNNER_INPUT_URL": "https://github.com/wisent-ai",
        "ACTIONS_RUNNER_INPUT_TOKEN": token,
        "ACTIONS_RUNNER_INPUT_NAME": RUNNER_NAME,
        "ACTIONS_RUNNER_INPUT_RUNNERGROUP": RUNNER_GROUP,
        "ACTIONS_RUNNER_INPUT_LABELS": RUNNER_LABELS,
        "ACTIONS_RUNNER_INPUT_WORK": "_work",
    }
    result = run(
        str(RUNNER_ROOT / "config.sh"),
        "--unattended",
        "--replace",
        "--disableupdate",
        check=False,
        env=environment,
        cwd=RUNNER_ROOT,
        preexec_fn=drop_privileges(uid, gid),
    )
    token = ""
    environment.pop("ACTIONS_RUNNER_INPUT_TOKEN", None)
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip()
        raise SystemExit(f"runner configuration failed: {detail}")
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


def install_hooks(uid: int, gid: int) -> tuple[pathlib.Path, pathlib.Path]:
    cleanup = RUNNER_ROOT / "clean-work.sh"
    cleanup.write_text(
        "#!/bin/sh\n"
        "set -eu\n"
        f"work={WORK_DIR!s}\n"
        'find "$work" -mindepth 1 -maxdepth 1 -exec rm -rf -- {} +\n',
        encoding="utf-8",
    )
    os.chown(cleanup, 0, 0)
    os.chmod(cleanup, 0o755)
    os.chown(WORK_DIR, uid, gid)
    os.chown(DIAG_DIR, uid, gid)

    PF_ANCHOR.parent.mkdir(parents=True, exist_ok=True)
    PF_ANCHOR.write_text(
        f"block return out quick proto {{ tcp udp }} from any "
        "to { 10.0.0.0/8, 100.64.0.0/10, 127.0.0.0/8, 169.254.0.0/16, "
        "172.16.0.0/12, 192.168.0.0/16, ::1/128, fc00::/7, fe80::/10 } "
        f"user {RUNNER_USER}\n",
        encoding="utf-8",
    )
    os.chown(PF_ANCHOR, 0, 0)
    os.chmod(PF_ANCHOR, 0o644)

    launcher = RUNNER_ROOT / "start-runner.sh"
    launcher.write_text(
        "#!/bin/sh\n"
        "set -eu\n"
        f"/sbin/pfctl -a {PF_ANCHOR_NAME} -f {PF_ANCHOR}\n"
        "/sbin/pfctl -E >/dev/null 2>&1 || true\n"
        f"exec /usr/bin/sudo -u {RUNNER_USER} -H -- /usr/bin/env "
        f"HOME={RUNNER_ROOT} "
        "PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin "
        f"ACTIONS_RUNNER_HOOK_JOB_STARTED={cleanup} "
        f"ACTIONS_RUNNER_HOOK_JOB_COMPLETED={cleanup} "
        f"{RUNNER_ROOT / 'run.sh'}\n",
        encoding="utf-8",
    )
    os.chown(launcher, 0, 0)
    os.chmod(launcher, 0o755)
    return cleanup, launcher


def install_service(cleanup: pathlib.Path, launcher: pathlib.Path) -> None:
    document = {
        "Label": LABEL,
        "ProgramArguments": [str(launcher)],
        "WorkingDirectory": str(RUNNER_ROOT),
        "EnvironmentVariables": {
            "ACTIONS_RUNNER_HOOK_JOB_STARTED": str(cleanup),
            "ACTIONS_RUNNER_HOOK_JOB_COMPLETED": str(cleanup),
            "HOME": str(RUNNER_ROOT),
            "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        },
        "RunAtLoad": True,
        "KeepAlive": True,
        "ThrottleInterval": 5,
        "ProcessType": "Background",
        "StandardOutPath": str(DIAG_DIR / "launchd.stdout.log"),
        "StandardErrorPath": str(DIAG_DIR / "launchd.stderr.log"),
    }
    with PLIST.open("wb") as output:
        plistlib.dump(document, output, sort_keys=True)
    os.chown(PLIST, 0, 0)
    os.chmod(PLIST, 0o644)
    run("/bin/launchctl", "bootout", "system", str(PLIST), check=False)
    run("/bin/launchctl", "bootstrap", "system", str(PLIST))
    run("/bin/launchctl", "enable", f"system/{LABEL}")
    run("/bin/launchctl", "kickstart", "-k", f"system/{LABEL}")


def main() -> int:
    require_root()
    if sys.platform != "darwin" or os.uname().machine != "arm64":
        raise SystemExit("this installer is pinned to the Apple Silicon fleet target")
    uid, gid = runner_identity()
    install_package(uid, gid)
    configure_runner(uid, gid)
    cleanup, launcher = install_hooks(uid, gid)
    install_service(cleanup, launcher)
    state = run("/bin/launchctl", "print", f"system/{LABEL}").stdout
    if "state = running" not in state:
        raise SystemExit("runner LaunchDaemon did not reach running state")
    print("runner service: running")
    print(f"runner identity: {RUNNER_USER} uid={uid}")
    print(f"runner labels: self-hosted,macOS,ARM64,{RUNNER_LABELS}")
    print("private-network egress: blocked")
    return 0


if __name__ == "__main__":
    sys.exit(main())
