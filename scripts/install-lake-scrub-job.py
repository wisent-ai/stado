#!/usr/bin/env python3
"""Install the vault-sourced Lake scrub as a scheduled LaunchAgent.

The scrub has to be standing, not a pass: every future transcript can carry a
credential as prose, and the streamer keeps importing older ones. This installs
`com.wisent.transcript-lake-secret-scrub` beside the streamer's own agent, in the
same `gui/<uid>` domain, so it runs as the user whose keyring unlocks the vault.

`HOME` is declared in the plist on purpose: launchd does not set it, and skarbiec
resolves its keyring through it, which fails in a way that reads like a missing
vault.

Writes the plist and reports the unit's state. Bootstrapping is left to the caller
(`launchctl bootstrap gui/<uid> <plist>`), because a helper running in the launchd
Background session cannot reach the GUI domain, and the same file works from either.
"""

import os
import pathlib
import subprocess
import sys

NONE = None
ZERO = len("")
HOME = pathlib.Path(os.path.expanduser("~"))
LABEL = "com.wisent.transcript-lake-secret-scrub"
AGENTS = HOME / "Library" / "LaunchAgents"
LOGS = HOME / "Library" / "Logs"
PLIST = AGENTS / f"{LABEL}.plist"
JOB = (
    HOME
    / "Documents"
    / "CodingProjects"
    / "Wisent"
    / "wisent-compute"
    / "scripts"
    / "scrub-lake-from-vault.py"
)
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


def main():
    os.umask(0o022)
    if not JOB.is_file():
        raise SystemExit(f"no job script at {JOB}")
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
