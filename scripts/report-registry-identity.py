#!/usr/bin/env python3
"""Say which registry document this host is actually reading, and how old it is.

Two stores can answer the same command: the authority's object API and a local
copy on an operator machine. When the adapter between them refuses, a reader can
silently fall back to the copy, and then an edit "lands" while the fleet never
sees it -- which is how a push built from a stale copy reverted two facts today.

Prints the directory generation, the digest of the document, and a couple of
facts that only recent edits carry, for whichever store this host resolves.
"""

import hashlib
import json
import os
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
STADO = HOME / ".stado" / "bin" / "stado"
CONFIG = pathlib.Path(os.environ.get("STADO_CONFIG", HOME / ".config" / "stado" / "config.json"))
MARKERS = ("account_ref", "placement")


def main():
    document = json.loads(CONFIG.read_text(encoding="utf-8")) if CONFIG.is_file() else {}
    print(f"storage.stado.url {((document.get('storage') or {}).get('stado') or {}).get('url', '(unset)')}")
    print(f"override          {os.environ.get('WC_STADO_STORAGE_URL', '(none)')}")
    pulled = subprocess.run(
        [str(STADO), "registry", "pull"], capture_output=True, text=True, check=False
    )
    if pulled.returncode != ZERO:
        raise SystemExit(f"pull failed: {(pulled.stderr or pulled.stdout).strip()[: len('a' * 160)]}")
    registry = json.loads(pulled.stdout)
    print(f"digest            {hashlib.sha256(pulled.stdout.encode()).hexdigest()[: len('a' * 16)]}")
    print(f"generation        {(registry.get('service_directory') or {}).get('generation')}")
    endpoints = ((registry.get("service_directory") or {}).get("services") or {}).get(
        "stado-object-api", {}
    ).get("endpoints", {})
    print(f"object-api hosts  {sorted(endpoints)}")
    for target in registry.get("targets", []):
        facts = {marker: target.get(marker) for marker in MARKERS if target.get(marker)}
        services = [item.get("name") for item in (target.get("services") or [])]
        print(
            f"  {str(target.get('name'))[: len('a' * 28)]:30}"
            f" services={len(services)} weles={'com.wisent.always-on.weles' in services} {facts or ''}"
        )
    return NONE


sys.exit(main())
