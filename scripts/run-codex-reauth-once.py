#!/usr/bin/env python3
"""Run the Codex reauth runner once, here, and show what it did.

The runner normally ticks from a LaunchAgent, and an agent job cannot be
kickstarted from an SSH session -- so a repair has no way to be exercised except
by running the same program the agent runs. This does that, with the worker
environment the agent would have, and prints the tail of both streams.

It performs no browser login: without a donor credential store the runner
refuses a fresh login and can only reuse a session already on this host.
"""

import os
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
RUNNER = HOME / "weles" / "scripts" / "trajectories" / "codex" / "reauth.mjs"
ENV_FILE = HOME / ".config" / "weles" / "worker.env"
NODE = "/opt/homebrew/bin/node"
TAIL = len("l" * 40)


def worker_env():
    values = {}
    if not ENV_FILE.is_file():
        return values
    for line in ENV_FILE.read_text(encoding="utf-8", errors="replace").splitlines():
        stripped = line.strip().removeprefix("export ").strip()
        name, separator, raw = stripped.partition("=")
        if separator and not stripped.startswith("#"):
            values[name.strip()] = raw.strip().strip('"').strip("'")
    return values


def main():
    if not RUNNER.is_file():
        raise SystemExit(f"no runner at {RUNNER}")
    environment = {
        **os.environ,
        **worker_env(),
        "HOME": str(HOME),
        "PATH": f"/opt/homebrew/bin:/usr/local/bin:{os.environ.get('PATH', '/usr/bin:/bin')}",
    }
    proc = subprocess.run(
        [NODE, str(RUNNER)],
        capture_output=True,
        text=True,
        check=False,
        cwd=str(HOME / "weles"),
        env=environment,
        timeout=float(len("s" * 300)),
    )
    print(f"exit {proc.returncode}")
    for stream, text in (("stdout", proc.stdout), ("stderr", proc.stderr)):
        lines = [line for line in text.splitlines() if line.strip()]
        print(f"== {stream} ({len(lines)} lines)")
        for line in lines[-TAIL:]:
            print(f"  {line[: len('a' * 200)]}")
    return NONE


sys.exit(main())
