#!/usr/bin/env python3
"""Measure a host that cannot reach the store, and hand its document in for it.

Measurement has to happen on the machine being measured -- that is the whole
point -- but publication does not. This fleet already made that split for the
other per-host document: `deploy/host_health_beacon.sh` has a collect-only mode
and `scripts/publish-linux-beacon-via-stado.sh` hands the result in, which is why
`host_health/ubuntu-server.json` is fresh in the store while that host cannot
reach the object API at all.

Capabilities take the same route. `measure-host-capabilities` prints its document
and publishes nothing, so this runs it through the registry SSH channel and PUTs
the result from a machine whose store is reachable. The document is the host's
own measurement, byte for byte, and it is keyed by the registry target name the
host resolved for itself, so nothing about the reading side changes.

Usage: hand-in-host-capabilities.py TARGET [TARGET ...]

This is not a replacement for the periodic unit on a host that can publish; it
is the path for a host that cannot, and the way to hand in one measurement now
without waiting for a network repair.
"""

import json
import os
import pathlib
import ssl
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
STADO = HOME / ".stado" / "bin" / "stado"
CONFIG = HOME / ".config" / "stado" / "config.json"
PREFIX = "host_capabilities"
HELPER = "measure-host-capabilities"
SCHEMA = "wisent.host-capabilities.v1"
# The remote measurement waits on a browser twice, and the SSH channel adds its
# own setup, so this bound is the remote bound with room around it.
MEASURE_TIMEOUT = len("a" * 240)
PUT_TIMEOUT = len("a" * 30)


def storage_settings():
    """The object API this machine is pointed at, from its own Stado config."""
    settings = json.loads(CONFIG.read_text(encoding="utf-8")).get("storage", {}).get("stado", {})
    for required in ("url", "namespace", "token_file"):
        if not str(settings.get(required, "")).strip():
            raise SystemExit(f"storage.stado.{required} is not set in {CONFIG}")
    return settings


def measure(target):
    """The host's own capability document, as the host printed it."""
    proc = subprocess.run(
        [str(STADO), "host", "run-helper", target, HELPER],
        capture_output=True,
        text=True,
        timeout=MEASURE_TIMEOUT,
        check=False,
    )
    if proc.returncode != ZERO:
        raise SystemExit(
            f"{target}: run-helper {HELPER} failed: "
            f"{proc.stderr.strip().splitlines()[-len('a'):] or proc.stdout.strip()}"
        )
    # `run-helper` prints the helper's stdout and nothing else on success, but a
    # transport that ever adds a line would silently corrupt the document, so the
    # JSON is located rather than assumed to start at byte zero.
    text = proc.stdout
    start = text.find("{")
    if start < ZERO:
        raise SystemExit(f"{target}: {HELPER} printed no JSON document")
    document = json.loads(text[start:])
    if document.get("schema") != SCHEMA:
        raise SystemExit(f"{target}: {HELPER} printed schema {document.get('schema')!r}")
    if document.get("host") != target:
        raise SystemExit(
            f"{target}: the host measured itself as {document.get('host')!r}; refusing to "
            "publish one host's measurement under another host's name"
        )
    return document


def put(settings, document):
    body = (json.dumps(document, indent=len("ba"), sort_keys=False) + "\n").encode("utf-8")
    uri = f"stado://{settings['namespace']}/{PREFIX}/{document['host']}.json"
    endpoint = f"{settings['url'].rstrip('/')}/api/object?" + urllib.parse.urlencode({"uri": uri})
    token = pathlib.Path(os.path.expanduser(settings["token_file"])).read_text(
        encoding="utf-8"
    ).strip()
    request = urllib.request.Request(endpoint, data=body, method="PUT")
    request.add_header("Authorization", f"Bearer {token}")
    request.add_header("Content-Type", "application/json")
    request.add_header("Content-Length", str(len(body)))
    context = NONE
    if settings["url"].startswith("https://"):
        context = ssl.create_default_context()
        if str(settings.get("ca_file", "")).strip():
            context.load_verify_locations(os.path.expanduser(settings["ca_file"]))
    try:
        with urllib.request.urlopen(request, timeout=PUT_TIMEOUT, context=context) as answer:
            return uri, len(body), answer.status, answer.read().decode("utf-8", "replace").strip()
    except urllib.error.HTTPError as error:
        raise SystemExit(
            f"PUT {uri} returned HTTP {error.code}: {error.read().decode('utf-8', 'replace')}"
        )
    except (urllib.error.URLError, OSError) as error:
        raise SystemExit(f"PUT {uri} could not reach {settings['url']}: {error}")


def main():
    targets = sys.argv[len("a"):]
    if not targets:
        raise SystemExit(__doc__.strip().splitlines()[-len("aaaaa")])
    settings = storage_settings()
    print(f"endpoint    {settings['url']}")
    for target in targets:
        document = measure(target)
        for name, capability in document["capabilities"].items():
            print(
                f"{target:<26} {name:<24} {str(capability['value']).lower():<5} "
                f"{capability['detail'][: len('a' * 150)]}"
            )
        uri, size, status, answer = put(settings, document)
        print(f"measured_at {document['measured_at']}")
        print(f"published   {uri}  {size} bytes  HTTP {status}  {answer[: len('a' * 120)]}")
    return NONE


sys.exit(main())
