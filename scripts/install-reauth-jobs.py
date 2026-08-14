#!/usr/bin/env python3
"""Install each reauth job in the launchd domain its work can actually run in.

This replaces `install-reauth-daemons.py`, which put all three jobs into
`/Library/LaunchDaemons` for one reason: the host had no console session, so
`launchctl` refused the login domain and nothing ticked. That made the schedule
work and the work impossible. A LaunchDaemon runs in the `Background` session,
which has no WindowServer, and two of these three jobs log in through a real
browser window. In that session Chromium starts, reaches the network and renders
pages -- so every probe passes -- and then dies the moment it creates a window,
in `ScopedCGWindowID::~ScopedCGWindowID`, which Playwright reports only as
`pwBrowser disconnected`.

So the domain is a consequence of the work, not of what happens to load:

  - a job that drives a browser is an Aqua LaunchAgent in `gui/<uid>`;
  - a job that only signs and posts is a LaunchDaemon in `system`.

A browser job whose graphical session does not exist is left uninstalled and
said out loud. Silently downgrading it to a daemon is what produced a year of
green schedules and dead logins.
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
UID = os.getuid()
OWNER_ONLY = 0o600
# Why each job needs what it needs, so the placement can be read rather than
# recalled: the browser jobs complete an OAuth consent screen in a Google
# window, the Codex job signs a request and posts it.
NEEDS_WINDOW = {
    "com.wisent.codex-reauth": False,
    "com.wisent.claude-reauth": True,
    "com.wisent.kimi-reauth": True,
}


def run(*args):
    proc = subprocess.run(args, capture_output=True, text=True, check=False)
    return proc.stdout + proc.stderr


def state(domain, label):
    text = run("/usr/bin/sudo", "-n", "/bin/launchctl", "print", f"{domain}/{label}") if domain == "system" \
        else run("/bin/launchctl", "print", f"{domain}/{label}")
    if not text.strip().startswith(f"{domain}/{label}"):
        return NONE
    found = re.search(r"^\s*state = (.+)$", text, re.MULTILINE)
    return found.group(len(["v"])).strip() if found else "loaded"


def graphical_domain_exists():
    text = run("/bin/launchctl", "print", f"gui/{UID}")
    return text.strip().startswith(f"gui/{UID}")


def source_document(label):
    for path in (AGENTS / f"{label}.plist", DAEMONS / f"{label}.plist",
                 AGENTS / f"{label}.plist.superseded-by-system-daemon"):
        if path.is_file():
            try:
                return path, plistlib.loads(path.read_bytes())
            except (OSError, ValueError):
                continue
    return NONE, NONE


def shaped(document, as_daemon):
    wanted = dict(document)
    label = wanted.get("Label", "reauth")
    environment = dict(wanted.get("EnvironmentVariables", {}))
    environment.setdefault("HOME", str(HOME))
    environment.setdefault("PATH", "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin")
    wanted["EnvironmentVariables"] = environment
    wanted.setdefault("WorkingDirectory", str(HOME))
    wanted.setdefault("StandardOutPath", str(LOGS / f"{label}.out"))
    wanted.setdefault("StandardErrorPath", str(LOGS / f"{label}.err"))
    if as_daemon:
        wanted["UserName"] = os.environ.get("USER", HOME.name)
        wanted.pop("LimitLoadToSessionType", NONE)
        return wanted
    # An agent inherits the user; naming the session type is what keeps it out
    # of the login-less domain this repair is about.
    wanted.pop("UserName", NONE)
    wanted["LimitLoadToSessionType"] = "Aqua"
    return wanted


def install_daemon(label, document):
    path = DAEMONS / f"{label}.plist"
    staging = HOME / ".stado" / "files" / f"{label}.plist"
    staging.parent.mkdir(parents=True, exist_ok=True)
    with staging.open("wb") as handle:
        plistlib.dump(document, handle)
    os.chmod(staging, OWNER_ONLY)
    run("/usr/bin/sudo", "-n", "/bin/cp", str(staging), str(path))
    run("/usr/bin/sudo", "-n", "/usr/sbin/chown", "root:wheel", str(path))
    run("/usr/bin/sudo", "-n", "/bin/chmod", "u=rw,go=r", str(path))
    run("/usr/bin/sudo", "-n", "/bin/launchctl", "bootout", f"system/{label}")
    time.sleep(len("a"))
    run("/usr/bin/sudo", "-n", "/bin/launchctl", "enable", f"system/{label}")
    detail = run("/usr/bin/sudo", "-n", "/bin/launchctl", "bootstrap", "system", str(path))
    return state("system", label), detail.strip()


def install_agent(label, document):
    AGENTS.mkdir(parents=True, exist_ok=True)
    path = AGENTS / f"{label}.plist"
    with path.open("wb") as handle:
        plistlib.dump(document, handle)
    os.chmod(path, 0o644)
    run("/bin/launchctl", "bootout", f"gui/{UID}/{label}")
    time.sleep(len("a"))
    run("/bin/launchctl", "enable", f"gui/{UID}/{label}")
    detail = run("/bin/launchctl", "bootstrap", f"gui/{UID}", str(path))
    return state(f"gui/{UID}", label), detail.strip()


def retire_daemon(label):
    path = DAEMONS / f"{label}.plist"
    if not path.is_file():
        return "no system daemon"
    run("/usr/bin/sudo", "-n", "/bin/launchctl", "bootout", f"system/{label}")
    run("/usr/bin/sudo", "-n", "/bin/rm", "-f", str(path))
    return "system daemon removed"


def main():
    LOGS.mkdir(parents=True, exist_ok=True)
    graphical = graphical_domain_exists()
    print(f"caller session   {run('/bin/launchctl', 'managername').strip()}")
    print(f"gui/{UID} domain  {'present' if graphical else 'absent -- nobody is logged in on this host'}")
    for label, needs_window in NEEDS_WINDOW.items():
        source, document = source_document(label)
        print(f"== {label} {'browser job' if needs_window else 'signing job'}")
        if not document:
            print("   no unit file anywhere; nothing to place")
            continue
        print(f"   source       {source}")
        if not needs_window:
            loaded, detail = install_daemon(label, shaped(document, as_daemon=True))
            print(f"   system       {loaded or 'not loaded'} {detail if not loaded else ''}")
            continue
        print(f"   retired      {retire_daemon(label)}")
        if not graphical:
            kept = AGENTS / f"{label}.plist.awaiting-graphical-session"
            AGENTS.mkdir(parents=True, exist_ok=True)
            with kept.open("wb") as handle:
                plistlib.dump(shaped(document, as_daemon=False), handle)
            print("   not loaded   this job opens a browser window and there is no Aqua session to open it in;")
            print(f"   staged       {kept} loads as soon as someone is logged in")
            continue
        loaded, detail = install_agent(label, shaped(document, as_daemon=False))
        print(f"   gui/{UID}     {loaded or 'not loaded'} {detail if not loaded else ''}")
    return NONE


sys.exit(main())
