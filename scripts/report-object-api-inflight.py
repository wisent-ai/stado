#!/usr/bin/env python3
"""Report the object API's in-flight requests and log tail on THIS host.

Read-only. Exists because a release submit's first store call hung with an
ESTABLISHED connection and no reply, from two different network paths — the
block is inside the serving process, and the only way to say WHERE is to look
from its side: which sockets it holds, how many threads it has, and what its
log said last. No secret is printed; log lines are tailed verbatim, and the
object API logs no bearer values.
"""

import glob
import os
import pathlib
import subprocess

HOME = pathlib.Path(os.path.expanduser("~"))


def run(*args):
    proc = subprocess.run(args, capture_output=True, text=True, check=False)
    return proc.stdout


def main():
    listener = run("/usr/sbin/lsof", "-nP", "-iTCP:8765")
    print("== polaczenia :8765")
    print(listener.strip() or "(none)")
    pids = {
        line.split()[1]
        for line in listener.splitlines()[1:]
        if line.strip() and "LISTEN" in line and line.split()[0] == "stado"
    }
    for pid in sorted(pids):
        print(f"== proces {pid}")
        print(run("/bin/ps", "-p", pid, "-o", "pid,etime,rss,command").strip())
        threads = run("/bin/ps", "-M", "-p", pid)
        print(f"   watki: {max(len(threads.splitlines()) - 1, 0)}")

    print("== ogony logow object api")
    candidates = sorted(
        glob.glob(str(HOME / ".stado" / "logs" / "*object-api*")), key=os.path.getmtime
    )
    if not candidates:
        candidates = sorted(
            glob.glob(str(HOME / ".stado" / "logs" / "*stado*")), key=os.path.getmtime
        )[-2:]
    for log in candidates[-2:]:
        print(f"-- {log}")
        lines = pathlib.Path(log).read_text(errors="replace").splitlines()
        for line in lines[-12:]:
            print(f"   {line[:220]}")


main()
