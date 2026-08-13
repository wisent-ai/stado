#!/usr/bin/env python3
"""Show what the scheduled reauth jobs actually run, with values redacted.

The runners are exercised by hand during a repair and by a launchd job in
production, and those are only the same thing if the job's launcher is read. It
may hold a lock, a log path, an environment file, or a node it expects on PATH.

Any assignment whose name looks like a credential is printed as its name and the
length of its value, never the value.
"""

import os
import pathlib
import re
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
ROOT = HOME / "weles" / "scripts" / "worker" / "deploy"
JOBS = ("codex-reauth", "claude-reauth", "kimi-reauth")
SECRET = re.compile(r"(KEY|SECRET|TOKEN|PASSWORD|PASS|CREDENTIAL)", re.IGNORECASE)
ASSIGNMENT = re.compile(r"^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)=(.*)$")


def redact(line):
    found = ASSIGNMENT.match(line)
    if not found:
        return line
    name, value = found.group(len(["n"])), found.group(len(["n", "v"]))
    if SECRET.search(name):
        return f"{name}=<{len(value.strip())} chars>"
    return line


def main():
    for job in JOBS:
        path = ROOT / job / "reauth-launch.sh"
        print(f"== {path} {'present' if path.is_file() else 'absent'}")
        if not path.is_file():
            continue
        for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
            stripped = line.strip()
            if not stripped or stripped.startswith("#"):
                continue
            print(f"  {redact(line)[: len('a' * 160)]}")
    # The launchers source their own env file, and which one decides whether the
    # runner takes the Supabase path or the Skarbiec path. Names only.
    for path in (
        HOME / "weles" / "var" / "worker.env",
        HOME / ".config" / "weles" / "worker.env",
    ):
        print(f"== {path} {'present' if path.is_file() else 'absent'}")
        if not path.is_file():
            continue
        names = []
        for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
            found = ASSIGNMENT.match(line)
            if found and not line.strip().startswith("#"):
                names.append(found.group(len(["n"])))
        interesting = [name for name in names if "SUPABASE" in name or "MODEL_ROUTER" in name]
        print(f"  variables {len(names)}; store-shaping {interesting or '(none)'}")
    return NONE


sys.exit(main())
