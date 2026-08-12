#!/usr/bin/env python3
"""Say where this host's health beacon writes, and whether it lands.

`registry beacon-age` reads `host_health/<host>.json` out of the canonical
store. A beacon that runs happily while that object never appears is writing
somewhere else -- the launcher carries its own environment, and a store the rest
of the fleet does not read makes a live host look dead.

Read-only: it prints the launcher, its store-shaping environment, and every
beacon object on disk with its age.
"""

import datetime
import json
import os
import pathlib
import re
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
LAUNCHER = HOME / ".stado" / "bin" / "host-health-beacon-launcher"
CONFIG = HOME / ".config" / "stado" / "config.json"
INTERESTING = ("WC_STORAGE_BACKEND", "WC_LOCAL_STORAGE_PATH", "WC_BUCKET", "STADO_CONFIG")


def age_of(path):
    seconds = datetime.datetime.now().timestamp() - path.stat().st_mtime
    return f"{seconds / len('a' * 60):.1f} min"


def main():
    if LAUNCHER.is_file():
        text = LAUNCHER.read_text(encoding="utf-8", errors="replace")
        print(f"launcher    {LAUNCHER}")
        for name in INTERESTING:
            for line in text.splitlines():
                if name in line:
                    print(f"  sets      {line.strip()[:120]}")
        commands = re.findall(r"stado\s+[a-z-]+(?:\s+[a-z-]+)?", text)
        print(f"  runs      {' | '.join(sorted(set(commands))) or '(no stado call found)'}")
    else:
        print(f"launcher    {LAUNCHER} (absent)")


    # The launcher points the beacon at a config of its own, so the host's
    # config says nothing about where the beacon writes. Read the one it names.
    own = HOME / ".stado" / "host-health-beacon.config.json"
    if own.is_file():
        settings = json.loads(own.read_text(encoding="utf-8"))
        storage = settings.get("storage", {})
        print(f"beacon cfg  {own}")
        print(
            f"  storage   backend {storage.get('backend')}  "
            f"url {storage.get('stado', {}).get('url')}  "
            f"local {storage.get('local', {}).get('path')}"
        )
    else:
        print(f"beacon cfg  {own} (absent)")
    # Publishing needs a bearer: either a token file named directly, or a
    # Skarbiec consumer grant. Show what this host actually holds, because
    # "beacon never reported" is usually one absent credential.
    tokens = sorted(path.name for path in (HOME / ".stado").glob("*token*") if path.is_file())
    print(f"token files {' '.join(tokens) or '(none)'}")
    if CONFIG.is_file():
        storage = json.loads(CONFIG.read_text(encoding="utf-8")).get("storage", {})
        print(f"config      backend {storage.get('backend')}  url {storage.get('stado', {}).get('url')}")

    found = ZERO
    for path in sorted((HOME / ".stado").rglob("host_health/*.json")):
        print(f"on disk     {path}  {age_of(path)} old")
        found += len(["one"])
    if not found:
        print("on disk     no beacon object anywhere under ~/.stado")
    return NONE


sys.exit(main())
