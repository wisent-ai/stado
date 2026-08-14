#!/usr/bin/env python3
"""Say whether the browser-login helpers can run on this host at all.

The reauth runners end at `login.mjs`, and when that exits without a blob the
error carries stack frames rather than a reason. Each login helper has a small
set of things it needs before it can drive anything: a provider CLI at a fixed
path, a node-pty build for the interpreter in use, a Chromium, and its own
credential store. Print which of those are present so a failure names a missing
file instead of a missing login.

Read-only.
"""

import json
import os
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
TREE = HOME / "weles"
NODE = pathlib.Path("/opt/homebrew/bin/node")
BINARIES = {
    "claude cli": HOME / ".local" / "bin" / "claude",
    "kimi cli": HOME / ".kimi-code" / "bin" / "kimi",
    "codex cli": HOME / ".local" / "bin" / "codex",
    "node": NODE,
}
MODULES = ("node-pty", "playwright", "playwright-core")


def module_state(name):
    proc = subprocess.run(
        [str(NODE), "-e", f"import('{name}').then(()=>console.log('ok'),e=>console.log(e.code||e.message))"],
        capture_output=True,
        text=True,
        check=False,
        cwd=str(TREE),
        env={**os.environ, "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"},
    )
    return (proc.stdout + proc.stderr).strip().splitlines()[-1:] or ["(no answer)"]


def main():
    for label, path in BINARIES.items():
        state = "present" if path.is_file() else "absent"
        version = ""
        if path.is_file() and os.access(path, os.X_OK):
            proc = subprocess.run(
                [str(path), "--version"], capture_output=True, text=True, check=False, timeout=float(len("s" * 20))
            )
            version = (proc.stdout or proc.stderr).strip().splitlines()[:1]
        print(f"{label:<12} {path} {state} {version}")

    for name in MODULES:
        print(f"module {name:<16} {module_state(name)}")

    keychain = subprocess.run(
        ["/usr/bin/security", "find-generic-password", "-s", "Claude Code-credentials"],
        capture_output=True,
        text=True,
        check=False,
    )
    print(f"keychain claude {'present' if keychain.returncode == ZERO else keychain.stderr.strip()[:80]}")
    return NONE


sys.exit(main())
