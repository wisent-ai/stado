#!/usr/bin/env python3
"""Publish host health into the store this host's readers actually use.

On a host whose beacon is a KeepAlive loop, the publish address is an `export`
in `~/.stado/bin/host-health-beacon-launcher`, not a launchd key. The authority's
copy still carried `http://127.0.0.1:18765`, a `stado host forward-local` install
that tunnels to the laptop's health API -- so the always-on machine published its
health into a laptop that sleeps, while every reader looked at the authority and
saw a host three minutes stale.

The address is not a matter of taste: it is `storage.stado.url` from this host's
own configuration, the same endpoint its capability publisher and its readers
use. This rewrites the export to that value, keeps a copy of the launcher,
refuses to write one that would not parse, and leaves the reload to the caller
so a running `/bin/sh` is replaced rather than re-reading an edited file.
"""

import json
import os
import pathlib
import re
import subprocess
import sys

NONE = None
HOME = pathlib.Path(os.path.expanduser("~"))
LAUNCHER = HOME / ".stado" / "bin" / "host-health-beacon-launcher"
CONFIG = HOME / ".config" / "stado" / "config.json"
KEY = "STADO_HOST_HEALTH_API_URL"


def configured_store():
    document = json.loads(CONFIG.read_text(encoding="utf-8"))
    url = ((document.get("storage") or {}).get("stado") or {}).get("url") or ""
    if not url:
        raise SystemExit(f"{CONFIG} names no storage.stado.url to publish to")
    return url.rstrip("/")


def main():
    if not LAUNCHER.is_file():
        raise SystemExit(f"no beacon launcher at {LAUNCHER}")
    original = LAUNCHER.read_text(encoding="utf-8")
    wanted = configured_store()
    pattern = re.compile(rf'^export {KEY}="?([^"\n]*)"?$', re.M)
    found = pattern.search(original)
    print(f"launcher   {LAUNCHER}")
    print(f"before     {KEY}={found.group(1) if found else '(not exported)'}")
    print(f"wanted     {KEY}={wanted}")
    if not found:
        raise SystemExit(f"{LAUNCHER} does not export {KEY}; not editing blind")
    if found.group(1) == wanted:
        print("settled    the beacon already publishes to its own store")
        return NONE
    body = pattern.sub(f'export {KEY}="{wanted}"', original, count=1)
    LAUNCHER.with_name(LAUNCHER.name + ".before-store").write_text(original, encoding="utf-8")
    LAUNCHER.write_text(body, encoding="utf-8")
    check = subprocess.run(["/bin/sh", "-n", str(LAUNCHER)], capture_output=True,
                           text=True, check=False)
    if check.returncode != 0:
        LAUNCHER.write_text(original, encoding="utf-8")
        raise SystemExit(f"edit refused, launcher restored: {check.stderr.strip()}")
    print("written    reload the unit for a fresh interpreter to read it")
    return NONE


sys.exit(main())
