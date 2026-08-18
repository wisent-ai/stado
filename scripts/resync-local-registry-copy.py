#!/usr/bin/env python3
"""Bring this host's local registry copy back in line with the authority.

Every push from this laptop lands at the authority through the resolver adapter,
and the adapter then refuses to serve: it bootstraps from the local copy of the
store, sees a service directory whose content no longer matches the generation it
cached, and exits `EX_UNAVAILABLE` rather than answer with a document it cannot
vouch for. That is the guard working, but nothing was updating the copy, so a
correct registry edit took the fleet's read path down until somebody realigned it
by hand -- three times today.

This is that repair, in one command: address the local copy directly (the adapter
is down by definition when this is needed), fetch the authority's document over
the helper channel that does not depend on the adapter, write it into the local
copy, and restart the resolver. The configuration is restored to the adapter at
the end whatever happens.
"""

import json
import os
import pathlib
import subprocess
import sys
import time
import urllib.parse

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
CONFIG = HOME / ".config" / "stado" / "config.json"
STADO = HOME / ".stado" / "bin" / "stado"
TOKEN = HOME / ".stado" / "wisent-queue-object-api-token"
AUTHORITY_HOST = os.environ.get("REGISTRY_AUTHORITY", "control-host")
RESOLVER = "com.wisent.stado-resolver"
URI = "stado://probierz/registry.json"
# The resolver bootstraps from the local store's file, not from the object API's
# namespaced key, and the two are different paths on disk: writing only through
# the API left this file at the old generation, which is the document the
# resolver then refused to reconcile.
SNAPSHOT = HOME / ".stado" / "local-storage" / "registry.json"
SETTLE = 30

def run(*args, stdin=NONE):
    return subprocess.run(args, capture_output=True, text=True, input=stdin, check=False)


def store_url(url=NONE):
    document = json.loads(CONFIG.read_text(encoding="utf-8"))
    current = ((document.get("storage") or {}).get("stado") or {}).get("url")
    if url is not NONE and url != current:
        document["storage"]["stado"]["url"] = url
        CONFIG.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    return current


def own_port():
    document = json.loads(CONFIG.read_text(encoding="utf-8"))
    port = (document.get("object_api") or {}).get("port")
    if not port:
        raise SystemExit(f"{CONFIG} names no object_api.port to write the copy through")
    return int(port)


def put_local(port, body):
    token = TOKEN.read_text(encoding="utf-8").strip() if TOKEN.is_file() else ""
    url = (
        f"http://127.0.0.1:{port}/api/object?uri={urllib.parse.quote(URI, safe='')}"
    )
    return run("/usr/bin/curl", "-s", "-m", "25", "-X", "PUT",
               "-H", f"Authorization: Bearer {token}",
               "-H", "Content-Type: application/json",
               "--data-binary", "@-", "-o", "/dev/null",
               "-w", "%{http_code}", url, stdin=body).stdout.strip()


def main():
    adapter = store_url()
    local = f"http://127.0.0.1:{own_port()}"
    print(f"adapter    {adapter}")
    print(f"local      {local}")
    store_url(local)
    try:
        emitted = run(str(STADO), "host", "run-helper", AUTHORITY_HOST, "emit-registry-document")
        if emitted.returncode != ZERO or not emitted.stdout.strip():
            raise SystemExit(
                f"{AUTHORITY_HOST} did not hand over its registry: "
                f"{(emitted.stderr or emitted.stdout).strip()[:120]}"
            )
        document = json.loads(emitted.stdout)
        print(f"authority  generation "
              f"{(document.get('service_directory') or {}).get('generation')}, "
              f"{len(document.get('targets', []))} targets")
        print(f"write copy http={put_local(own_port(), emitted.stdout)}")
        if SNAPSHOT.is_file():
            SNAPSHOT.with_name(SNAPSHOT.name + ".before-resync").write_text(
                SNAPSHOT.read_text(encoding="utf-8"), encoding="utf-8"
            )
        SNAPSHOT.parent.mkdir(parents=True, exist_ok=True)
        SNAPSHOT.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
        print(f"write file {SNAPSHOT}")
    finally:
        store_url(adapter)
    run("/bin/launchctl", "kickstart", "-k", f"gui/{os.getuid()}/{RESOLVER}")
    time.sleep(SETTLE)
    token = TOKEN.read_text(encoding="utf-8").strip() if TOKEN.is_file() else ""
    probe = run("/usr/bin/curl", "-s", "-m", "20", "-o", "/dev/null", "-w", "%{http_code}",
                "-H", f"Authorization: Bearer {token}",
                f"{adapter}/api/object?uri={urllib.parse.quote(URI, safe='')}")
    print(f"adapter    registry http={probe.stdout.strip()}")
    return NONE


sys.exit(main())
