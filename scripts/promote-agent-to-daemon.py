#!/usr/bin/env python3
"""Run a declared always-on service on a host that has no login session.

This host is a headless authority: nobody is logged in, so `gui/501` does not
exist and `launchctl bootstrap gui/501` answers "Domain does not support
specified action". Two services were installed as user agents there --
`weles-keyword-planner-api` and `weles-echo-api` -- so they could never load, and
the registry declared them while nothing listened.

A service that must run without a session belongs in the system domain, where
this host's Skarbiec, Weles and Brama already are. This copies the agent's plist
into `/Library/LaunchDaemons`, adds the `UserName` the agent ran as so file and
credential paths keep resolving, bootstraps it, and reports whether it stayed up.
The original agent plist is kept beside itself with a `.was-agent` suffix, so one
`mv` undoes this.

Refuses silently-broken outcomes: if the daemon does not reach `running`, the
daemon plist is removed again and the failure is printed with the log tail, so a
host is never left with two definitions of one service.
"""

import os
import pathlib
import plistlib
import subprocess
import sys
import time

NONE = None
# The two services this host declares and could never load as user agents.
# `PROMOTE_LABEL` overrides the pair for any other service in the same bind.
DEFAULT_LABELS = (
    "com.wisent.compute.service.weles-keyword-planner-api",
    "com.wisent.weles-echo-api",
)
LABELS = [os.environ["PROMOTE_LABEL"]] if os.environ.get("PROMOTE_LABEL") else list(DEFAULT_LABELS)
USER = os.environ.get("PROMOTE_USER", "charles")
AGENTS = pathlib.Path("/Users") / USER / "Library" / "LaunchAgents"
DAEMONS = pathlib.Path("/Library/LaunchDaemons")
LOGS = pathlib.Path("/Users") / USER / ".stado" / "logs"
# A Node service performs three signed Skarbiec acquisitions before it listens,
# so a verdict taken after a few seconds calls a healthy start a failure.
SETTLE = 30


def run(*args):
    proc = subprocess.run(args, capture_output=True, text=True, check=False)
    return (proc.stdout + proc.stderr).strip()


def state_of(label):
    text = run("/usr/bin/sudo", "-n", "/bin/launchctl", "print", f"system/{label}")
    rows = [line.strip() for line in text.splitlines()
            if "state = " in line or "last exit code" in line]
    return " | ".join(rows[:2]) or "no such unit"


def log_tail(label):
    for name in (f"{label}.err", f"{label}.log"):
        path = LOGS / name
        if path.is_file():
            lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
            if lines:
                return f"{name}: {lines[-1].strip()[:110]}"
    return "no log"


def as_booleans(document):
    """Old ASCII plists have no boolean type: `RunAtLoad = true` is the string.

    launchd in a user domain tolerates that; in the system domain the job simply
    never runs -- `runs = 0`, no error, nothing in any log. Coerce the keys whose
    meaning is a flag, at the top level and inside `KeepAlive`.
    """
    flags = ("RunAtLoad", "AbandonProcessGroup", "LaunchOnlyOnce", "Debug",
             "SessionCreate", "EnableTransactions")
    def coerce(value):
        if isinstance(value, str) and value.lower() in ("true", "false"):
            return value.lower() == "true"
        return value
    for key in flags:
        if key in document:
            document[key] = coerce(document[key])
    keep = document.get("KeepAlive")
    if isinstance(keep, str):
        document["KeepAlive"] = coerce(keep)
    elif isinstance(keep, dict):
        document["KeepAlive"] = {name: coerce(value) for name, value in keep.items()}
    return document


def promote(label):
    agent = AGENTS / f"{label}.plist"
    daemon = DAEMONS / f"{label}.plist"
    print(f"service    {label}")
    if daemon.is_file():
        print(f"  settled  already a daemon: {state_of(label)}")
        return
    if not agent.is_file():
        print(f"  absent   no agent plist at {agent}")
        return
    try:
        document = plistlib.loads(agent.read_bytes())
    except Exception:
        # Some units on this host are written in the old NeXTSTEP ASCII plist
        # format, which launchd reads and `plistlib` does not. Convert a copy
        # rather than declare a working service unreadable.
        converted = subprocess.run(
            ["/usr/bin/plutil", "-convert", "xml1", "-o", "-", str(agent)],
            capture_output=True, check=False,
        )
        try:
            document = plistlib.loads(converted.stdout)
        except Exception as problem:
            print(f"  refused  {agent.name} does not parse: {str(problem)[:64]}")
            return
    document = as_booleans(document)
    document.setdefault("UserName", USER)
    document.pop("LimitLoadToSessionType", NONE)
    staged = pathlib.Path("/tmp") / f"{label}.plist"
    with staged.open("wb") as handle:
        plistlib.dump(document, handle)
    run("/usr/bin/sudo", "-n", "/bin/cp", str(staged), str(daemon))
    run("/usr/bin/sudo", "-n", "/usr/sbin/chown", "root:wheel", str(daemon))
    run("/usr/bin/sudo", "-n", "/bin/chmod", "644", str(daemon))
    staged.unlink(missing_ok=True)
    booted = run("/usr/bin/sudo", "-n", "/bin/launchctl", "bootstrap", "system", str(daemon))
    print(f"  bootstrap {booted[:60] or 'accepted'}")
    time.sleep(SETTLE)
    settled = state_of(label)
    print(f"  state    {settled}")
    if "state = running" not in settled and "state = waiting" not in settled:
        run("/usr/bin/sudo", "-n", "/bin/launchctl", "bootout", f"system/{label}")
        run("/usr/bin/sudo", "-n", "/bin/rm", "-f", str(daemon))
        print(f"  reverted the daemon did not stay up; {log_tail(label)}")
        return
    agent.rename(agent.with_name(f"{agent.name}.was-agent"))
    print(f"  retired  {agent.name} -> {agent.name}.was-agent")


def main():
    for label in LABELS:
        promote(label)
    return NONE


sys.exit(main())
