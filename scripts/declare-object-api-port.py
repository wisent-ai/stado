#!/usr/bin/env python3
"""Write down the port the object API is already listening on.

The launcher used to derive its bind from `storage.stado.url`, a client address
that only coincides with this service on the registry authority. Replacing that
with an explicit `object_api.port` needs a value, and the honest source for it is
the running listener rather than anybody's memory: measure, then declare.

Idempotent. Refuses when no listener can be found, because a guess written into
config is how this class of defect started.
"""

import json
import os
import pathlib
import re
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
CONFIG = pathlib.Path(os.environ.get("STADO_CONFIG", HOME / ".config" / "stado" / "config.json"))
ARGUMENT = re.compile(r"--port\s+(\d+)")


def listening_ports():
    """The port the object API process itself was started with.

    Matching any `stado` listener is not enough: this host also runs the
    host-health API and a resolver that publishes a dozen adapter ports, and
    picking one of those would move the object API on top of another service.
    The service's own argv is unambiguous.
    """
    listing = subprocess.run(
        ["/bin/ps", "-Ao", "pid,command"], capture_output=True, text=True, check=False
    ).stdout
    found = {}
    for line in listing.splitlines():
        if "dashboard" not in line or "--port" not in line:
            continue
        match = ARGUMENT.search(line)
        if not match:
            continue
        found[int(match.group(len("a")))] = line.split()[ZERO]
    return found


def main():
    if not CONFIG.is_file():
        raise SystemExit(f"no config at {CONFIG}")
    document = json.loads(CONFIG.read_text(encoding="utf-8"))
    declared = (document.get("object_api") or {}).get("port")
    ports = listening_ports()
    print(f"declared   object_api.port = {declared if declared else '(absent)'}")
    print(f"listening  {sorted(ports.items()) or 'nothing named stado'}")
    adapters = {
        str(entry.get("bind", "")).rsplit(":", len("a"))[-1]
        for entry in (document.get("service_resolver") or {}).get("adapters") or []
    }
    candidates = [port for port in sorted(ports) if str(port) not in adapters]
    if not candidates:
        raise SystemExit("no listener that is not a resolver adapter; nothing to declare")
    chosen = candidates[ZERO]
    if declared == chosen:
        print("settled    the declaration already matches the listener")
        return NONE
    document.setdefault("object_api", {})["port"] = chosen
    CONFIG.write_text(json.dumps(document, indent=4) + "\n", encoding="utf-8")
    print(f"declared   object_api.port = {chosen} (measured, adapters excluded: {sorted(adapters) or 'none'})")
    return NONE


sys.exit(main())
