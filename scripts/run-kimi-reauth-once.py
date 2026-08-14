#!/usr/bin/env python3
"""Run the Kimi reauth job once, on its own, and show both streams.

Kickstarting all three jobs at once makes their output interleave and their
failures share blame -- one run showed a refused loopback connection while the
gateway was answering. Running this one alone, through its own launcher, keeps
the environment identical and the evidence attributable.
"""

import os
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
LAUNCHER = HOME / "weles" / "scripts" / "worker" / "deploy" / "kimi-reauth" / "reauth-launch.sh"
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
        timeout=float(len("s" * 600)),
    )
    print(f"exit {proc.returncode}")
    for stream, text in (("stdout", proc.stdout), ("stderr", proc.stderr)):
        lines = [line for line in text.splitlines() if line.strip()]
        print(f"== {stream} ({len(lines)} lines)")
        for line in lines[-TAIL:]:
            print(f"  {line[: len('a' * 400)]}")
    return NONE


sys.exit(main())
