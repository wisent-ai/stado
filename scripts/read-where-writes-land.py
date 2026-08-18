#!/usr/bin/env python3
"""Point this host's readers at the store the whole fleet shares.

`storage.stado.url` decides where `stado registry doctor`, `beacon-age` and every
other reader on this host look, and two endpoints answer for what looks like the
same store: this host's own object API (`object_api.port`) and the resolver
adapter that forwards to the authority.

Freshness cannot decide between them, and trying it made things worse. The local
endpoint served a *newer* host health document than the authority did, because
the authority's beacon publishes into this laptop through an old
`stado host forward-local` install -- so the local copy is ahead for one document
family and nineteen hours behind for another. Picking the newer answer just
rotates which host looks broken.

Identity decides. A probe written through a candidate is readable through that
candidate only if it is a private copy; the fleet's store is the one whose writes
the authority can see. This writes a probe, checks which endpoint the *other*
candidate can read it from, and configures the endpoint that is shared.
"""

import json
import os
import pathlib
import subprocess
import sys
import time
import urllib.parse

NONE = None
HOME = pathlib.Path(os.path.expanduser("~"))
CONFIG = HOME / ".config" / "stado" / "config.json"
TOKEN = HOME / ".stado" / "wisent-queue-object-api-token"
PROBE = "stado://probierz/diagnostics/store-identity-probe.json"


def token():
    return TOKEN.read_text(encoding="utf-8").strip() if TOKEN.is_file() else ""


def call(port, method, payload=NONE):
    url = (
        f"http://127.0.0.1:{port}/api/object?"
        f"uri={urllib.parse.quote(PROBE, safe='')}"
    )
    args = ["/usr/bin/curl", "-s", "-m", "15", "-H", f"Authorization: Bearer {token()}"]
    if method == "PUT":
        args += ["-X", "PUT", "-H", "Content-Type: application/json", "--data", payload]
    proc = subprocess.run(args + [url], capture_output=True, text=True, check=False)
    return proc.stdout


def candidates(document):
    """This host's own object API, and whatever the configuration addresses."""
    ports = []
    own = (document.get("object_api") or {}).get("port")
    if own:
        ports.append(int(own))
    configured = ((document.get("storage") or {}).get("stado") or {}).get("url") or ""
    parsed = urllib.parse.urlparse(configured)
    if parsed.port and parsed.port not in ports:
        ports.append(parsed.port)
    return ports


def shared_port(ports):
    """The endpoint whose writes another endpoint can also read is the shared one."""
    stamp = str(int(time.time()))
    for port in ports:
        if "200" not in call(port, "PUT", json.dumps({"probe": stamp})) and not call(port, "GET"):
            continue
        call(port, "PUT", json.dumps({"probe": stamp}))
        time.sleep(len("abc"))
        others = [other for other in ports if other != port]
        seen = [other for other in others if stamp in call(other, "GET")]
        print(f"port {port}   probe visible from {seen or 'nowhere else'}")
        if seen:
            return port
    return NONE


def main():
    document = json.loads(CONFIG.read_text(encoding="utf-8"))
    ports = candidates(document)
    if len(ports) < len("ab"):
        print(f"settled    one candidate only: {ports}")
        return NONE
    shared = shared_port(ports)
    if shared is NONE:
        # No endpoint's write showed up anywhere else: every candidate is a
        # private copy, and the adapter is the only one with a path off-host.
        adapter = ports[-1]
        print(f"private    no shared write seen; keeping the adapter {adapter}")
        shared = adapter
    wanted = f"http://127.0.0.1:{shared}"
    before = ((document.get("storage") or {}).get("stado") or {}).get("url")
    print(f"before     {before}")
    print(f"shared     {wanted}")
    if before == wanted:
        print("settled    readers already use the shared store")
        return NONE
    document["storage"]["stado"]["url"] = wanted
    CONFIG.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    print(f"after      {wanted}")
    return NONE


sys.exit(main())
