#!/usr/bin/env python3
"""Run the Claude reauth runner once, through its own launcher, and show output.

The job's log carries one line -- `FAILED: fetch failed` -- which names neither
the step nor the address. Running the launcher directly separates the streams and
keeps the environment the job has, including the Supabase credentials the
launcher acquires for itself.

It performs no browser login: without a donor credential row the runner refuses
a fresh login and can only reuse a session already on this host.
"""

import os
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
LAUNCHER = HOME / "weles" / "scripts" / "worker" / "deploy" / "claude-reauth" / "reauth-launch.sh"
TAIL = len("l" * 40)


def main():
    if not LAUNCHER.is_file():
        raise SystemExit(f"no launcher at {LAUNCHER}")
    proc = subprocess.run(
        ["/bin/bash", str(LAUNCHER)],
        capture_output=True,
        text=True,
        check=False,
        cwd=str(HOME / "weles"),
        env={
            **os.environ,
            "HOME": str(HOME),
            "PATH": f"/opt/homebrew/bin:/usr/local/bin:{os.environ.get('PATH', '/usr/bin:/bin')}",
        },
        timeout=float(len("s" * 240)),
    )
    print(f"exit {proc.returncode}")
    for stream, text in (("stdout", proc.stdout), ("stderr", proc.stderr)):
        lines = [line for line in text.splitlines() if line.strip()]
        print(f"== {stream} ({len(lines)} lines)")
        for line in lines[-TAIL:]:
            print(f"  {line[: len('a' * 220)]}")
    return NONE


sys.exit(main())
