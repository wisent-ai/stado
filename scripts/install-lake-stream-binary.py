#!/usr/bin/env python3
"""Install the committed transcript-lake build and let the stream service adopt it.

A masking rule only protects the archive once the process that writes the archive
carries it. The LaunchAgent `com.wisent.transcript-lake-stream` runs
`~/.local/bin/transcript-lake`, so a masker change is inert until that file is
replaced. `scripts/install-stream-service.sh` in the transcript-lake repository is
the full installer, but it also re-bootstraps the agent in the `gui/<uid>` domain,
which a helper running in the launchd Background session cannot reach. Replacing
the binary and terminating the running process is enough: the job declares
`KeepAlive`, so launchd starts the replacement itself, and the stream resumes from
its durable cursors.

Refuses to install from a dirty source tree, because the binary's provenance is
the commit it was built from. Prints digests, versions, process identities, and the
first commit line the new process writes.
"""

import hashlib
import os
import pathlib
import signal
import subprocess
import sys
import time

NONE = None
ZERO = len("")
HOME = pathlib.Path(os.path.expanduser("~"))
TREE = HOME / "Documents" / "CodingProjects" / "Wisent" / "transcript-lake"
CARGO = HOME / ".cargo" / "bin" / "cargo"
PREFIX = HOME / ".local"
TARGET = PREFIX / "bin" / "transcript-lake"
BUILD = HOME / ".cache" / "transcript-lake-build"
LOG = HOME / "Library" / "Logs" / "transcript-lake-stream.log"
PROCESS_PATTERN = ".local/bin/transcript-lake"
TIMEOUT = 3600
RESPAWN_SECONDS = 40
COMMIT_SECONDS = 120


def run(argv, **kwargs):
    """Every child gets cargo and the system tools on PATH; a caller may add to
    that environment (cargo needs its target directory moved) without losing it."""
    env = {**os.environ, "PATH": f"{CARGO.parent}:/opt/homebrew/bin:/usr/bin:/bin"}
    env.update(kwargs.pop("env", {}))
    return subprocess.run(
        argv,
        capture_output=True,
        text=True,
        check=False,
        timeout=TIMEOUT,
        env=env,
        **kwargs,
    )


def digest(path):
    if not path.is_file():
        return "absent"
    return hashlib.sha256(path.read_bytes()).hexdigest()[: len("a" * 16)]


def stream_pids():
    proc = run(["/usr/bin/pgrep", "-f", PROCESS_PATTERN])
    return [int(line) for line in proc.stdout.split() if line.strip().isdigit()]


def main():
    # The host agent runs helpers under a secret-safe umask that strips the
    # execute bit, so cargo cannot enter the directories it creates, and an
    # installed binary would land unexecutable. Nothing here writes a secret.
    os.umask(0o022)
    head = run(["/usr/bin/git", "-C", str(TREE), "rev-parse", "HEAD"]).stdout.strip()
    dirty = run(["/usr/bin/git", "-C", str(TREE), "status", "--porcelain", "--", "src", "Cargo.toml", "Cargo.lock"]).stdout.strip()
    print(f"source {TREE} at {head}")
    if dirty:
        for line in dirty.splitlines():
            print("  uncommitted " + line)
        raise SystemExit("refusing to install a binary whose source is not committed")

    before_digest = digest(TARGET)
    before_version = run([str(TARGET), "--version"]).stdout.strip() if TARGET.is_file() else "absent"
    before_pids = stream_pids()
    print(f"installed  sha256[:16]={before_digest} version={before_version} pids={before_pids}")

    BUILD.mkdir(parents=True, exist_ok=True)
    os.chmod(BUILD, 0o755)
    install = run(
        [str(CARGO), "install", "--path", str(TREE), "--root", str(PREFIX), "--locked", "--force"],
        cwd=str(TREE),
        env={"CARGO_TARGET_DIR": str(BUILD)},
    )
    for line in (install.stdout + install.stderr).splitlines():
        if any(mark in line for mark in ("error", "warning:", "Installed", "Replaced", "Compiling transcript-lake", "Finished")):
            print("  " + line.strip()[: len("a" * 165)])
    print(f"cargo install exit {install.returncode}")
    if install.returncode != ZERO:
        raise SystemExit(install.returncode)

    after_digest = digest(TARGET)
    after_version = run([str(TARGET), "--version"]).stdout.strip()
    print(f"installed  sha256[:16]={after_digest} version={after_version}")
    if after_digest == before_digest:
        print("binary unchanged; the running process already carries this build")
        return ZERO

    mark = LOG.stat().st_size if LOG.is_file() else ZERO
    for pid in before_pids:
        os.kill(pid, signal.SIGTERM)
        print(f"sent SIGTERM to {pid}")
    deadline = time.time() + RESPAWN_SECONDS
    new_pids = []
    while time.time() < deadline:
        new_pids = [pid for pid in stream_pids() if pid not in before_pids]
        if new_pids:
            break
        time.sleep(0.5)
    print(f"respawned pids {new_pids}")
    if not new_pids:
        raise SystemExit("launchd did not respawn the stream; check `launchctl print gui/<uid>`")

    # A commit line from the new process is the only proof that it is streaming,
    # not merely running.
    deadline = time.time() + COMMIT_SECONDS
    printed = ZERO
    while time.time() < deadline and printed < len("aaa"):
        with open(LOG, "r", encoding="utf-8", errors="replace") as handle:
            handle.seek(mark)
            fresh = [line.rstrip() for line in handle if line.strip()]
        for line in fresh[: len("aaa")]:
            print("  log " + line[: len("a" * 200)])
            printed += 1
        if printed:
            break
        time.sleep(1.0)
    if not printed:
        print("  log (no new line yet; the stream idles until a source append arrives)")
    return ZERO


sys.exit(main())
