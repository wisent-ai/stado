#!/usr/bin/env python3
"""Say which part of the browser never arrives, and what the bootstrap says.

The main thread sits idle in its message pump, which means it is waiting for
something that never reports back. A browser is a family of processes that find
each other through the per-user Mach bootstrap server, and this context prints
`NSNotificationCenter connection invalid` before anything else -- the sentence a
process gets when that server is not in its namespace.

This launches with Chromium's own logging on, lists the children it managed to
start, and asks launchd which bootstrap namespace the caller is in.
"""

import os
import pathlib
import re
import signal
import subprocess
import sys
import time

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
STATE = HOME / ".local" / "state" / "weles"
PAGE = "data:text/html,<title>probe</title><h1>probe</h1>"
SETTLE = 15
INTERESTING = re.compile(
    r"(bootstrap|mach|service|renderer|gpu|network|zygote|sandbox|launch|utility|Fail|error|ERROR)",
    re.IGNORECASE,
)


def newest_binary():
    found = sorted((HOME / ".local" / "share" / "weles-chromium").glob("*/Chromium.app/Contents/MacOS/Chromium"))
    if not found:
        raise SystemExit("no Weles Chromium on this host")
    return found[-1]


def children(pid):
    proc = subprocess.run(
        ["/bin/ps", "-Ao", "pid,ppid,comm"], capture_output=True, text=True, check=False
    )
    rows = []
    for line in proc.stdout.splitlines()[1:]:
        parts = line.split(NONE, 2)
        if len(parts) == len("abc") and parts[1] == str(pid):
            rows.append((parts[0], parts[2]))
    return rows


def bootstrap_state():
    print("== bootstrap")
    for command in (
        ["/bin/launchctl", "managername"],
        ["/bin/launchctl", "manageruid"],
        ["/bin/launchctl", "managerpid"],
    ):
        proc = subprocess.run(command, capture_output=True, text=True, check=False)
        print(f"   {command[-1]:12} {proc.stdout.strip() or proc.stderr.strip()}")
    for service in ("com.apple.distributed_notifications@Uv3", "com.apple.windowserver.active", "com.apple.CoreDisplay.master"):
        proc = subprocess.run(
            ["/bin/launchctl", "print", f"system/{service}"], capture_output=True, text=True, check=False
        )
        first = (proc.stdout or proc.stderr).strip().splitlines()[:1]
        print(f"   {service:44} {first[0][: len('a' * 60)] if first else '(silent)'}")


def main():
    binary = newest_binary()
    print(f"binary {binary}")
    bootstrap_state()
    log = STATE / "headless-verbose.log"
    STATE.mkdir(parents=True, exist_ok=True)
    with log.open("w", encoding="utf-8") as sink:
        proc = subprocess.Popen(
            [
                str(binary),
                "--headless=new",
                "--no-sandbox",
                "--disable-gpu",
                "--enable-logging=stderr",
                "--v=1",
                f"--user-data-dir={STATE / 'children-probe'}",
                f"--screenshot={STATE / 'children-probe.png'}",
                PAGE,
            ],
            stdout=sink,
            stderr=subprocess.STDOUT,
        )
        time.sleep(SETTLE)
        rows = children(proc.pid)
        alive = proc.poll()
        try:
            os.kill(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    print(f"== launch pid {proc.pid} exit {alive if alive is not NONE else 'still running'}")
    print(f"   children {len(rows)}")
    for pid, command in rows:
        print(f"     {pid:>7} {command[: len('a' * 120)]}")
    lines = [line.rstrip() for line in log.read_text(encoding="utf-8", errors="replace").splitlines() if line.strip()]
    print(f"== log {len(lines)} lines")
    picked = [line for line in lines if INTERESTING.search(line)]
    for line in (picked or lines)[: len("a" * 24)]:
        print(f"   {line[: len('a' * 150)]}")
    return NONE


sys.exit(main())
