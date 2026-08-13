#!/usr/bin/env python3
"""Point this host's object API at the Skarbiec the registry declares.

The object API verifies every caller's token against Skarbiec before it will
serve an object. Its unit carried `http://127.0.0.1:8787`, an address the
service directory does not name and that an unmanaged process squats on, so the
verifier failed and the API answered `503 object authorization unavailable` to
everything -- including the resolver's own refresh, which then let the service
directory cache go stale and made the fleet's registry unreadable. Hosts fell
back to reading `registry.json` off their own disks, which is the drift.

The declared endpoint is the one true answer, so read it from the registry
rather than restating a port here. Idempotent: a unit that already names the
declared endpoint is left untouched.
"""

import datetime
import json
import os
import pathlib
import plistlib
import subprocess
import time
import socket
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
STADO = HOME / ".stado" / "bin" / "stado"
LABEL = "com.wisent.always-on.stado-object-api"
PLIST = pathlib.Path("/Library/LaunchDaemons") / f"{LABEL}.plist"
SERVICE = "skarbiec"
KEYS = ("WC_OBJECT_SKARBIEC_URL", "WC_SKARBIEC_URL")
OWNER_ONLY = 0o600


def run(*args, check=False):
    proc = subprocess.run(args, capture_output=True, text=True, check=False)
    if check and proc.returncode != ZERO:
        raise SystemExit(f"{' '.join(args)} failed: {proc.stderr.strip() or proc.stdout}")
    return proc.stdout + proc.stderr


def registry_document():
    """The registry, read the way a repair tool must be able to read it.

    Asking the object API for the document that says how to fix the object API
    is a loop with no exit: the pull returns 503 for exactly the reason this
    script exists to remove. The host's own last-known-good copy is the way
    out, and it is the same document until someone writes a newer one.
    """
    text = run(str(STADO), "registry", "pull")
    try:
        return json.loads(text)
    except ValueError:
        for candidate in (
            HOME / ".stado" / "local-storage" / "registry.json",
            HOME / ".stado" / "local-storage" / "ecosystem" / "probierz" / "registry.json",
        ):
            if candidate.is_file():
                print(f"fallback    {candidate}")
                return json.loads(candidate.read_text(encoding="utf-8"))
        raise SystemExit("the object API is down and this host holds no registry copy")


def this_target(document):
    """Which registry target this machine is, without asking the object API.

    `registry self` answers this, and it is the right answer everywhere except
    here: it reads the store, the store is the thing that is down, and the
    repair then cannot name the host it is running on. The document already
    carries every hostname, so match against it.
    """
    node = socket.gethostname().lower()
    short = node.split(".")[ZERO]
    for entry in document.get("targets", []):
        names = [str(name).lower() for name in entry.get("hostnames", [])]
        names.append(str(entry.get("name", "")).lower())
        if any(name == node or name.split(".")[ZERO] == short for name in names if name):
            return entry.get("name")
    raise SystemExit(f"no registry target matches this machine ({node})")


def declared_endpoint():
    document = registry_document()
    here = this_target(document)
    service = document["service_directory"]["services"][SERVICE]
    endpoint = service.get("endpoints", {}).get(here)
    if not endpoint:
        raise SystemExit(f"the registry declares no {SERVICE} endpoint on {here}")
    return endpoint["url"]

def loaded():
    printed = run("/usr/bin/sudo", "-n", "/bin/launchctl", "print", f"system/{LABEL}")
    return printed.strip().startswith(f"system/{LABEL}")


def bootstrap():
    """Load the unit, tolerating launchd's teardown window.

    `bootout` returns before launchd has finished tearing the job down, and a
    `bootstrap` that lands in that window fails with "Input/output error" --
    which once left the unit rewritten with nothing running, a worse state than
    the one the repair started from.
    """
    detail = ""
    for _ in range(len("aaaaaaaaaa")):
        if loaded():
            return "already loaded"
        detail = run("/usr/bin/sudo", "-n", "/bin/launchctl", "bootstrap", "system", str(PLIST))
        if loaded():
            return detail.strip() or "ok"
        time.sleep(len("aa"))
    return detail.strip() or "did not load"


def main():
    if not PLIST.is_file():
        raise SystemExit(f"no unit at {PLIST}")
    document = plistlib.loads(PLIST.read_bytes())
    environment = document.get("EnvironmentVariables", {})
    wanted = declared_endpoint()
    current = {key: environment.get(key) for key in KEYS}
    print(f"unit        {PLIST}")
    print(f"declared    {wanted}")
    print(f"configured  {json.dumps(current)}")
    if all(value == wanted for value in current.values()):
        # Settled is about the file, and the file is not the service: a repair
        # that rewrote the unit and failed to load it lands here on the next
        # run, so loading is checked every time rather than assumed.
        print(f"bootstrap   {bootstrap()}")
        print(f"running     {'yes' if loaded() else 'no'}")
        return NONE

    for key in KEYS:
        environment[key] = wanted
    document["EnvironmentVariables"] = environment
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    staging = HOME / ".stado" / "files" / f"{LABEL}.{stamp}.plist"
    staging.parent.mkdir(parents=True, exist_ok=True)
    with staging.open("wb") as handle:
        plistlib.dump(document, handle)
    os.chmod(staging, OWNER_ONLY)

    run("/usr/bin/sudo", "-n", "/bin/cp", str(PLIST), f"{PLIST}.before-{stamp}")
    run("/usr/bin/sudo", "-n", "/bin/cp", str(staging), str(PLIST), check=True)
    run("/usr/bin/sudo", "-n", "/usr/sbin/chown", "root:wheel", str(PLIST))
    run("/usr/bin/sudo", "-n", "/bin/chmod", "644", str(PLIST))
    run("/usr/bin/sudo", "-n", "/bin/launchctl", "bootout", f"system/{LABEL}")
    print(f"backup      {PLIST}.before-{stamp}")
    print(f"bootstrap   {bootstrap()}")
    print(f"running     {'yes' if loaded() else 'no'}")
    return NONE


sys.exit(main())
