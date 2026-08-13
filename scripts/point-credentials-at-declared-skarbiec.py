#!/usr/bin/env python3
"""Point this host's credential store at the Skarbiec the registry declares.

`credential_store::read_item_with` uses the configured store URL in preference
to whatever coordinates a caller passes, so one wrong line in `skarbiec.url`
silently redirects every credential read on the host. On the always-on Mac it
named `127.0.0.1:17602` -- the Weles vault's adapter, declared for consumer
`weles` -- so the health beacon's own grant was offered to a vault that does not
hold it and to an adapter that refuses the connection. The beacon published
nothing for a day while the fleet listed its services as active.

The registry already says where this host's Skarbiec is. Read it from there.
Idempotent, and the previous config is kept beside the new one.
"""

import datetime
import json
import os
import pathlib
import socket
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
CONFIG = HOME / ".config" / "stado" / "config.json"
STADO = HOME / ".stado" / "bin" / "stado"
SERVICE = "skarbiec"
OWNER_ONLY = 0o600


def registry_document():
    proc = subprocess.run(
        [str(STADO), "registry", "pull"], capture_output=True, text=True, check=False
    )
    if proc.returncode == ZERO:
        return json.loads(proc.stdout)
    for candidate in (
        HOME / ".stado" / "local-storage" / "registry.json",
        HOME / ".stado" / "local-backup" / "registry.json",
    ):
        if candidate.is_file():
            print(f"fallback   {candidate}")
            return json.loads(candidate.read_text(encoding="utf-8"))
    raise SystemExit("no registry is readable from this host")


def this_target(document):
    node = socket.gethostname().lower()
    short = node.split(".")[ZERO]
    for entry in document.get("targets", []):
        names = [str(name).lower() for name in entry.get("hostnames", [])]
        names.append(str(entry.get("name", "")).lower())
        if any(name == node or name.split(".")[ZERO] == short for name in names if name):
            return entry.get("name")
    raise SystemExit(f"no registry target matches this machine ({node})")


def main():
    document = registry_document()
    here = this_target(document)
    service = document["service_directory"]["services"][SERVICE]
    endpoint = service.get("endpoints", {}).get(here, {}).get("url")
    if not endpoint:
        raise SystemExit(f"the registry declares no {SERVICE} endpoint on {here}")

    settings = json.loads(CONFIG.read_text(encoding="utf-8")) if CONFIG.is_file() else {}
    # The binding is `secrets.skarbiec.url`, and its compiled default is
    # `http://127.0.0.1:17602` -- a port that on this fleet belongs to the Weles
    # vault's adapter. A host that never states the key inherits that default
    # and reads its credentials from the wrong vault without ever saying so.
    holder = settings.get("secrets", {}).get("skarbiec", {})
    current = holder.get("url")
    stray = settings.get("skarbiec", {}).get("url")
    print(f"host       {here}")
    print(f"declared   {endpoint}")
    print(f"configured {current or '(unset, compiled default 127.0.0.1:17602)'}")
    if current == endpoint and not stray:
        print("settled    the credential store already names the declared Skarbiec")
        return NONE

    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    saved = CONFIG.with_name(f"{CONFIG.name}.before-skarbiec-url-{stamp}")
    saved.write_text(CONFIG.read_text(encoding="utf-8"), encoding="utf-8")
    os.chmod(saved, OWNER_ONLY)
    settings.setdefault("secrets", {}).setdefault("skarbiec", {})["url"] = endpoint
    # An earlier pass of this script wrote a top-level `skarbiec` key that no
    # binding reads. Two spellings of one setting is the drift this repairs.
    settings.pop("skarbiec", NONE)
    staging = CONFIG.with_name(f"{CONFIG.name}.{os.getpid()}.tmp")
    staging.write_text(json.dumps(settings, indent=len("ba")) + "\n", encoding="utf-8")
    os.chmod(staging, OWNER_ONLY)
    staging.replace(CONFIG)
    print(f"backup     {saved}")
    print(f"set        secrets.skarbiec.url {endpoint}")
    return NONE


sys.exit(main())
