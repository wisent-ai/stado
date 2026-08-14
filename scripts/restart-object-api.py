#!/usr/bin/env python3
"""Restart the object API in place and prove it came back before letting go.

This process serves the canonical registry, the release channel and every
capability object to the whole fleet, and it reads its namespace policy once at
startup (`config::object_api_namespaces()` is a LazyLock), so a policy change is
inert until it restarts. That makes the restart necessary and also the riskiest
thing in this repair.

`launchctl kickstart -k` restarts a loaded job without a window in which the job
does not exist, unlike an unload followed by a bootstrap that can fail halfway
and leave nothing running -- the failure mode that turned a degraded host into a
down host here once already.

Prints the before state, the restart, and the after state: pid, health, and a
read of one object behind each newly granted prefix. Exits non-zero if the
service does not answer again.
"""

import json
import os
import pathlib
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
LABEL = "com.wisent.always-on.stado-object-api"
BASE = "http://127.0.0.1:8765"
TOKEN_FILE = HOME / ".stado" / "wisent-queue-object-api-token"
PROBES = (
    "stado://probierz/registry.json",
    "stado://probierz/host_capabilities/control-host.json",
    "stado://probierz/job_requirements/weles-trajectories.json",
)
SETTLE = 45


def run(*args):
    proc = subprocess.run(args, capture_output=True, text=True, check=False)
    return proc.stdout + proc.stderr


def pid_of():
    for line in run("/bin/launchctl", "print", f"system/{LABEL}").splitlines():
        stripped = line.strip()
        if stripped.startswith("pid = "):
            return stripped[len("pid = "):]
    return "(none)"


def ask(uri):
    token = TOKEN_FILE.read_text(encoding="utf-8").strip() if TOKEN_FILE.is_file() else ""
    request = urllib.request.Request(
        f"{BASE}/api/object?uri={urllib.parse.quote(uri, safe='')}",
        headers={"Authorization": f"Bearer {token}"},
    )
    try:
        with urllib.request.urlopen(request, timeout=20) as answer:
            return answer.status, len(answer.read())
    except urllib.error.HTTPError as error:
        return error.code, len(error.read() or b"")
    except OSError as error:
        return f"unreachable ({error})", ZERO


def health():
    try:
        with urllib.request.urlopen(f"{BASE}/healthz", timeout=10) as answer:
            return answer.status
    except urllib.error.HTTPError as error:
        return error.code
    except OSError as error:
        return f"unreachable ({error})"


def report(when):
    print(f"{when:7} pid {pid_of()} healthz {health()}")
    for uri in PROBES:
        status, size = ask(uri)
        print(f"        {status:>14} {size:>8} bytes  {uri}")


def main():
    report("before")
    detail = run("/usr/bin/sudo", "-n", "/bin/launchctl", "kickstart", "-k", f"system/{LABEL}")
    print(f"restart {detail.strip() or 'accepted'}")
    deadline = time.time() + SETTLE
    while time.time() < deadline:
        if health() == 200:
            break
        time.sleep(len("ab"))
    report("after")
    if health() != 200:
        raise SystemExit("the object API did not answer again; restore it before doing anything else")
    return NONE




sys.exit(main())
