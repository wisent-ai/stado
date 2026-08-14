#!/usr/bin/env python3
"""Sample the hung headless browser and name what it is waiting on.

With its own profile directory the browser no longer crashes: it starts, prints
`NSNotificationCenter connection invalid`, and then never returns. A hang has an
address -- the frames the main thread is blocked in -- and `sample` prints them
while the process is still stuck, which a crash report cannot do because no
crash occurs.

Two launches are sampled: one plain, one told not to use the login keychain.
The difference between them is the answer.
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
SETTLE = 12
SAMPLE_SECONDS = 3
BLOCKING = re.compile(r"(Sec\w+|Keychain|__ulock_wait|mach_msg2?_trap|semaphore_wait|bootstrap_look_up|XPC|read)")


def newest_binary():
    found = sorted((HOME / ".local" / "share" / "weles-chromium").glob("*/Chromium.app/Contents/MacOS/Chromium"))
    if not found:
        raise SystemExit("no Weles Chromium on this host")
    return found[-1]


def attempt(binary, label, extra):
    profile = STATE / f"stall-probe-{label}"
    shot = STATE / f"stall-probe-{label}.png"
    if shot.is_file():
        shot.unlink()
    proc = subprocess.Popen(
        [
            str(binary),
            "--headless=new",
            "--no-sandbox",
            "--disable-gpu",
            f"--user-data-dir={profile}",
            f"--screenshot={shot}",
            *extra,
            PAGE,
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    print(f"== {label} pid {proc.pid} extra {' '.join(extra) or '(none)'}")
    deadline = time.time() + SETTLE
    while time.time() < deadline:
        if proc.poll() is not NONE:
            break
        time.sleep(1)
    finished = proc.poll()
    size = shot.stat().st_size if shot.is_file() else ZERO
    if finished is not NONE:
        print(f"   exited {finished} wrote {size} bytes")
        return NONE
    print(f"   still running after {SETTLE}s, wrote {size} bytes -- sampling")
    sample = subprocess.run(
        ["/usr/bin/sample", str(proc.pid), str(SAMPLE_SECONDS), "-mayDie"],
        capture_output=True,
        text=True,
        check=False,
        timeout=90,
    )
    text = sample.stdout
    main = text.partition("Binary Images")[0]
    interesting = [line.rstrip() for line in main.splitlines() if BLOCKING.search(line)]
    for line in interesting[: len("a" * 18)]:
        print(f"     {line.strip()[: len('a' * 145)]}")
    if not interesting:
        for line in main.splitlines()[: len("a" * 25)]:
            if line.strip():
                print(f"     {line.strip()[: len('a' * 145)]}")
    try:
        os.kill(proc.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    return NONE


def main():
    binary = newest_binary()
    print(f"binary {binary}")
    attempt(binary, "plain", [])
    attempt(binary, "mock-keychain", ["--use-mock-keychain", "--password-store=basic"])
    return NONE


sys.exit(main())
