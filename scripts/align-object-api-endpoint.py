#!/usr/bin/env python3
"""Point a host's object-API endpoint at the port that host actually serves.

`service_directory.services.stado-object-api.endpoints.operator-host` named
`127.0.0.1:18776`, which is that host's resolver ADAPTER -- a client-side stable
port, not a service. Two things followed: the adapter resolved to itself, and
somebody later wrote a launcher that derived the service's bind from the client
URL, which took the store down on the operator laptop.

The endpoint is now taken from the host's own declaration (`object_api.port` in
its config, which `declare-object-api-port.py` measures from the running
listener), so the directory says where the service is rather than where its
clients happen to knock.

Compare-and-swap safe; prints the before and after and nothing else.
"""

import json
import os
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
STADO = HOME / ".stado" / "bin" / "stado"
SERVICE = os.environ.get("ALIGN_SERVICE", "stado-object-api")
TARGET = os.environ.get("ALIGN_TARGET", "operator-host")
PORT = os.environ.get("ALIGN_PORT", "")
STAGED = HOME / ".stado" / "registry-align-endpoint.json"


def run(*args):
    return subprocess.run(args, capture_output=True, text=True, check=False)


def local_port():
    if PORT:
        return PORT
    config = pathlib.Path(os.environ.get("STADO_CONFIG", HOME / ".config" / "stado" / "config.json"))
    document = json.loads(config.read_text(encoding="utf-8")) if config.is_file() else {}
    port = (document.get("object_api") or {}).get("port")
    if not port:
        raise SystemExit("no object_api.port declared here; run declare-object-api-port first")
    return str(port)


def main():
    port = local_port()
    pulled = run(str(STADO), "registry", "pull")
    if pulled.returncode != ZERO:
        raise SystemExit(f"registry pull failed: {(pulled.stderr or pulled.stdout).strip()[:160]}")
    document = json.loads(pulled.stdout)
    services = (document.get("service_directory") or {}).get("services") or {}
    entry = services.get(SERVICE)
    if entry is NONE:
        raise SystemExit(f"the directory declares no service {SERVICE}")
    endpoints = entry.setdefault("endpoints", {})
    wanted = f"http://127.0.0.1:{port}"
    before = (endpoints.get(TARGET) or {}).get("url")
    directory = document.get("service_directory") or {}
    generation = directory.get("generation")
    bump_only = "--bump-only" in sys.argv
    print(f"service    {SERVICE}")
    print(f"target     {TARGET}")
    print(f"before     {before or '(absent)'} (directory generation {generation})")
    if before == wanted and not bump_only:
        print("settled    the directory already names this host's own listener")
        return NONE
    endpoints[TARGET] = {"url": wanted}
    # A reader refuses a directory that changed without advancing its
    # generation -- and it is right to: that is how a stale cache and a fresh
    # one become indistinguishable. An edit here is a change there.
    if isinstance(generation, int):
        directory["generation"] = generation + len("a")
        print(f"generation {generation} -> {directory['generation']}")
    STAGED.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    validated = run(str(STADO), "registry", "validate", str(STAGED))
    print(f"validate   {(validated.stdout or validated.stderr).strip().splitlines()[-1:]}")
    if validated.returncode != ZERO:
        raise SystemExit("the edited registry does not validate; nothing was pushed")
    pushed = run(str(STADO), "registry", "push", str(STAGED))
    print(f"push       {(pushed.stdout or pushed.stderr).strip().splitlines()[-1:]}")
    if pushed.returncode != ZERO:
        raise SystemExit("push refused; the canonical document is unchanged")
    STAGED.unlink()
    print(f"after      {wanted}")
    return NONE


sys.exit(main())
