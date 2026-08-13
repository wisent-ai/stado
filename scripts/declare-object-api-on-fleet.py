#!/usr/bin/env python3
"""Move the fleet's object API off the operator's laptop and declare it.

`service_directory.services["stado-object-api"]` named `operator-host` as the
active host, published the operator's dashboard port as the only endpoint, and
pointed `managed_service` at the Weles unit. The always-on host meanwhile runs
`com.wisent.always-on.stado-object-api` and nothing in the directory said so, so
every host that could not reach the operator's laptop fell back to reading a
`registry.json` off its own disk. Two documents then drift with one name, which
is how a resolver started against a registry that had not been written in weeks.

This declares the real thing: the always-on host is the active host, its control
plane is the endpoint, the unit that serves it is the managed service, and the
operator is a consumer with an adapter of its own so it stops being the server.

Idempotent, and it writes only through `stado registry push`, which validates
the whole document first.
"""

import json
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
STADO = pathlib.Path.home() / ".stado" / "bin" / "stado"
SERVICE = "stado-object-api"
ACTIVE_HOST = "control-host"
FLEET_URL = "http://127.0.0.1:8765"
OPERATOR = "operator-host"
DELIVERY = pathlib.Path("/tmp/registry-object-api.json")


def stado(*args, stdin=NONE):
    proc = subprocess.run(
        [str(STADO), *args], capture_output=True, text=True, input=stdin, check=False
    )
    if proc.returncode != ZERO:
        raise SystemExit(f"stado {' '.join(args)} failed: {proc.stderr.strip() or proc.stdout}")
    return proc.stdout


def target(document, name):
    for entry in document.get("targets", []):
        if entry.get("name") == name:
            return entry
    raise SystemExit(f"{name} is not a target in this registry")


def main():
    document = json.loads(stado("registry", "pull"))
    directory = document["service_directory"]
    services = directory["services"]
    service = services.setdefault(SERVICE, {})

    before = json.dumps(service, sort_keys=True)
    service["active_host"] = ACTIVE_HOST
    service["endpoints"] = {ACTIVE_HOST: {"url": FLEET_URL}}
    service["managed_service"] = "com.wisent.always-on.stado-object-api"
    consumers = service.setdefault("consumers", {})
    for consumer in ("operator", "weles"):
        consumers.setdefault(consumer, {"capabilities": ["object-storage"]})

    # An adapter for this service is a trap rather than a convenience: the
    # resolver reads the registry to learn its routes, so a host whose store is
    # an adapter the resolver serves cannot start at all. Off-host callers take
    # the tailnet address directly. Remove one if an earlier pass declared it.
    adapters = target(document, OPERATOR).setdefault("service_resolver", {}).setdefault(
        "adapters", []
    )
    kept = [entry for entry in adapters if entry.get("service") != SERVICE]
    removed = len(adapters) - len(kept)
    target(document, OPERATOR)["service_resolver"]["adapters"] = kept

    after = json.dumps(service, sort_keys=True)
    if before == after and not removed:
        print("settled     the directory already names the always-on host")
        return NONE

    # Consumers cache the directory by generation, so a change that leaves the
    # counter alone is a change some of them will never see.
    directory["generation"] = int(directory.get("generation", ZERO)) + len(["bump"])

    body = json.dumps(document, indent=len("ba")) + "\n"
    DELIVERY.write_text(body, encoding="utf-8")
    print(stado("registry", "validate", str(DELIVERY)).strip())
    print(stado("registry", "push", str(DELIVERY)).strip())
    print(f"delivery    {DELIVERY}")
    print(f"active host {ACTIVE_HOST}  endpoint {FLEET_URL}")
    print(f"generation  {directory['generation']}  adapters removed {removed}")
    return NONE


sys.exit(main())
