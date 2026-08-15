#!/usr/bin/env python3
"""Install the vault-sourced Lake scrub as a scheduled LaunchAgent.

The scrub has to be standing, not a pass: every future transcript can carry a
credential as prose, and the streamer keeps importing older ones. This installs
`com.wisent.transcript-lake-secret-scrub` beside the streamer's own agent, in the
same `gui/<uid>` domain, so it runs as the user whose keyring unlocks the vault.

`HOME` is declared in the plist on purpose: launchd does not set it, and skarbiec
resolves its keyring through it, which fails in a way that reads like a missing
vault.

A LaunchAgent cannot read ~/Documents: macOS answers `Operation not permitted` with
no prompt, because the job holds no Documents grant and granting one would change
this host's security posture. So the two scripts are COPIED to
`~/.local/libexec/transcript-lake-scrub/` and the unit runs the copies, the same
separation the streamer has between its checkout and `~/.local/bin`. Digests of both
are reported, so a stale copy is visible rather than silent; re-running this installs
the current source.

Writes the copies and the plist, then reports the unit's state. Bootstrapping is left
to the caller (`launchctl bootstrap gui/<uid> <plist>`), because a helper running in
the launchd Background session cannot reach the GUI domain, and the same file works
from either.
"""

import hashlib
import os
import pathlib
import shutil
import subprocess
import sys

NONE = None
ZERO = len("")
HOME = pathlib.Path(os.path.expanduser("~"))
LABEL = "com.wisent.transcript-lake-secret-scrub"
AGENTS = HOME / "Library" / "LaunchAgents"
LOGS = HOME / "Library" / "Logs"
PLIST = AGENTS / f"{LABEL}.plist"
LIBEXEC = HOME / ".local" / "libexec" / "transcript-lake-scrub"
WISENT = HOME / "Documents" / "CodingProjects" / "Wisent"
# (source in the repository, installed name) — the job first, its scrub second.
SOURCES = (
    (WISENT / "wisent-compute" / "scripts" / "scrub-lake-from-vault.py", "scrub-lake-from-vault.py"),
    (WISENT / "transcript-lake" / "scripts" / "scrub-known-secret.py", "scrub-known-secret.py"),
)
JOB = LIBEXEC / SOURCES[ZERO][1]

LOG = LOGS / "transcript-lake-secret-scrub.log"
# Hourly. The streamer imports continuously, and a secret that reached the archive
# should not sit there for a day; a full pass measured under three minutes.
INTERVAL_SECONDS = 60 * 60

TEMPLATE = """<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>/usr/bin/python3</string>
        <string>-u</string>
        <string>{job}</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
        <key>HOME</key>
        <string>{home}</string>
        <key>LAKE_DATA</key>
        <string>{lake}</string>
        <key>PATH</key>
        <string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin</string>
    </dict>
    <key>StartInterval</key>
    <integer>{interval}</integer>
    <key>RunAtLoad</key>
    <false/>
    <key>ProcessType</key>
    <string>Background</string>
    <key>LowPriorityIO</key>
    <true/>
    <key>Nice</key>
    <integer>5</integer>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
</dict>
</plist>
"""


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()[: len("a" * 8)]


def main():
    os.umask(0o022)
    for source, _ in SOURCES:
        if not source.is_file():
            raise SystemExit(f"no source at {source}")
    LIBEXEC.mkdir(parents=True, exist_ok=True)
    os.chmod(LIBEXEC, 0o755)
    for source, name in SOURCES:
        installed = LIBEXEC / name
        fresh = not installed.is_file() or digest(installed) != digest(source)
        if fresh:
            shutil.copyfile(source, installed)
            os.chmod(installed, 0o755)
        print(f"{'installed' if fresh else 'unchanged'} {installed} sha256[:8]={digest(installed)}"
              f" from {source}")
    AGENTS.mkdir(parents=True, exist_ok=True)
    LOGS.mkdir(parents=True, exist_ok=True)
    content = TEMPLATE.format(
        label=LABEL,
        job=JOB,
        home=HOME,
        lake=HOME / ".transcript-lake",
        interval=INTERVAL_SECONDS,
        log=LOG,
    )
    existing = PLIST.read_text(encoding="utf-8") if PLIST.is_file() else NONE
    if existing == content:
        print(f"plist unchanged {PLIST}")
    else:
        PLIST.write_text(content, encoding="utf-8")
        print(f"plist written {PLIST} ({len(content)} bytes)")
    print(f"job {JOB}")
    print(f"log {LOG}")
    print(f"interval {INTERVAL_SECONDS}s")
    state = subprocess.run(
        ["/bin/launchctl", "print", f"gui/{os.getuid()}/{LABEL}"],
        capture_output=True,
        text=True,
        check=False,
    )
    if state.returncode == ZERO:
        for line in state.stdout.splitlines():
            if any(
                key in line
                for key in ("state =", "last exit code", "runs =", "program =", "path =")
            ):
                print("  " + line.strip())
    else:
        print(f"  not bootstrapped yet: launchctl bootstrap gui/{os.getuid()} {PLIST}")
    return ZERO


sys.exit(main())
