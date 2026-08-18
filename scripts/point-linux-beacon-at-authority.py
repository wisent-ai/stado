#!/usr/bin/env python3
"""Publish this Linux host's beacon into the store the fleet reads.

`stado-host-beacon.service` posted to a laptop's tailnet name. That name still
answers, so nothing looked broken -- but a laptop serves its own private copy of
the store, and every reader resolves host health at the authority. The host's own
document therefore never reached the fleet, and the only writer that landed was
the relay running on that laptop, whose collector carried a shorter unit list. A
service installed and started here read as missing for as long as the relay kept
winning.

The endpoint belongs to the authority. This writes a systemd drop-in rather than
editing the shipped unit, so the change is visible, reversible with one `rm`, and
survives a package update. Verifies the endpoint answers before it switches, and
restarts the timer's service once.
"""

import os
import pathlib
import subprocess
import sys
import time

NONE = None
UNIT = "stado-host-beacon.service"
DROP_IN_DIR = pathlib.Path("/etc/systemd/system") / f"{UNIT}.d"
DROP_IN = DROP_IN_DIR / "10-authority-endpoint.conf"
AUTHORITY = os.environ.get("BEACON_AUTHORITY_URL", "https://100.120.25.24:8765")
CA = pathlib.Path("/root/.stado/stado-tailnet-ca.crt")
BODY = """[Service]
Environment=STADO_HOST_HEALTH_API_URL={authority}
"""


def run(*args):
    proc = subprocess.run(args, capture_output=True, text=True, check=False)
    return (proc.stdout + proc.stderr).strip()


def answers(url):
    args = ["/usr/bin/curl", "-s", "-m", "10", "-o", "/dev/null", "-w", "%{http_code}"]
    if url.startswith("https") and CA.is_file():
        args += ["--cacert", str(CA)]
    return run(*args, f"{url}/healthz")


def current_endpoint():
    shown = run("/bin/systemctl", "show", "-p", "Environment", UNIT)
    for token in shown.replace("Environment=", " ").split():
        if token.startswith("STADO_HOST_HEALTH_API_URL="):
            return token.split("=", 1)[1]
    return "(unset)"


def main():
    before = current_endpoint()
    print(f"before     {before}")
    print(f"authority  {AUTHORITY} http={answers(AUTHORITY) or 'no answer'}")
    if answers(AUTHORITY) not in ("200", "401", "404"):
        raise SystemExit(f"{AUTHORITY} does not answer; leaving the beacon alone")
    if before == AUTHORITY:
        print("settled    the beacon already publishes to the authority")
        return NONE
    body = BODY.format(authority=AUTHORITY)
    staged = pathlib.Path("/tmp/10-authority-endpoint.conf")
    staged.write_text(body, encoding="utf-8")
    run("/usr/bin/sudo", "-n", "/bin/mkdir", "-p", str(DROP_IN_DIR))
    run("/usr/bin/sudo", "-n", "/bin/cp", str(staged), str(DROP_IN))
    staged.unlink(missing_ok=True)
    run("/usr/bin/sudo", "-n", "/bin/systemctl", "daemon-reload")
    print(f"drop-in    {DROP_IN}")
    print(f"start      {run('/usr/bin/sudo', '-n', '/bin/systemctl', 'start', UNIT) or 'accepted'}")
    time.sleep(len("abcdefghij"))
    print(f"after      {current_endpoint()}")
    return NONE


sys.exit(main())
