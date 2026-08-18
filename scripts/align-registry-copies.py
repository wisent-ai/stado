#!/usr/bin/env python3
"""Make every registry copy on this host say what the object API says.

A push updates the store's namespaced object (`ecosystem/probierz/registry.json`)
and leaves the plain `registry.json` files beside it untouched. Store-backed
readers open those, so one host ends up holding several generations of the same
document at once -- measured here today: 3, 9, 10, 10 and 10 on the authority,
8 and 10 on the laptop. The resolver compares what it reads with what it cached,
sees content that changed while the generation did not, and refuses to serve
rather than guess. That refusal is correct, and it disabled the fleet's read path
three times.

This aligns them: read the document the host's own object API serves -- the one
`push` does update -- and write it into every `registry.json` under `~/.stado`
whose content differs, keeping a `.before-align` copy of each. Idempotent; prints
each file and the generation it moved from.
"""

import json
import os
import pathlib
import subprocess
import sys
import urllib.parse

NONE = None
HOME = pathlib.Path(os.path.expanduser("~"))
CONFIG = HOME / ".config" / "stado" / "config.json"
TOKEN = HOME / ".stado" / "wisent-queue-object-api-token"
URI = "stado://probierz/registry.json"
# Only the store's live copies. A glob for `registry.json` under `~/.stado` also
# finds build stages, checked-out work trees and `.metadata` sidecars, and an
# earlier run of this script rewrote 62 of them on the authority before they were
# restored from its own backups. Those are inputs to builds and store bookkeeping,
# not documents a reader resolves, and they are none of this repair's business.
LIVE_COPIES = (
    pathlib.Path(".stado") / "registry.json",
    pathlib.Path(".stado") / "work" / "registry.json",
    pathlib.Path(".stado") / "local-storage" / "registry.json",
    pathlib.Path(".stado") / "local-storage" / "ecosystem" / "probierz" / "registry.json",
    pathlib.Path(".stado") / "local-backup" / "registry.json",
    pathlib.Path(".stado") / "local-backup" / "ecosystem" / "probierz" / "registry.json",
)

def authority_document():
    document = json.loads(CONFIG.read_text(encoding="utf-8"))
    port = (document.get("object_api") or {}).get("port")
    if not port:
        raise SystemExit(f"{CONFIG} names no object_api.port to read from")
    token = TOKEN.read_text(encoding="utf-8").strip() if TOKEN.is_file() else ""
    url = (
        f"http://127.0.0.1:{port}/api/object?"
        f"uri={urllib.parse.quote(URI, safe='')}"
    )
    proc = subprocess.run(
        ["/usr/bin/curl", "-s", "-m", "20", "-H", f"Authorization: Bearer {token}", url],
        capture_output=True, text=True, check=False,
    )
    try:
        served = json.loads(proc.stdout)
    except json.JSONDecodeError:
        raise SystemExit(f"the object API on {port} did not serve the registry")
    if "targets" not in served:
        raise SystemExit(f"the object API on {port} served no targets")
    return served


def generation_of(path):
    try:
        return (json.loads(path.read_text(encoding="utf-8")).get("service_directory")
                or {}).get("generation")
    except Exception:
        return "unreadable"


def main():
    wanted = authority_document()
    body = json.dumps(wanted, indent=2) + "\n"
    generation = (wanted.get("service_directory") or {}).get("generation")
    print(f"authority  generation {generation}, {len(wanted.get('targets', []))} targets")
    for relative in LIVE_COPIES:
        path = HOME / relative
        if not path.is_file():
            print(f"  absent   {str(path).replace(str(HOME), '~')}")
            continue
        if path.read_text(encoding="utf-8") == body:
            print(f"  settled  {str(path).replace(str(HOME), '~')}")
            continue
        was = generation_of(path)
        path.with_name(path.name + ".before-align").write_text(
            path.read_text(encoding="utf-8"), encoding="utf-8"
        )
        path.write_text(body, encoding="utf-8")
        print(f"  aligned  gen {was} -> {generation}  {str(path).replace(str(HOME), '~')}")
    return NONE


sys.exit(main())
