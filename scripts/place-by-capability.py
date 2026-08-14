#!/usr/bin/env python3
"""Answer "which host can actually do this work" from measurements, not belief.

For weeks the fleet placed browser logins on `charless-mac-mini` because that is
the always-on host, and every layer above re-diagnosed the resulting crash as
its own bug: launchd's `Background` session has no WindowServer, so headed
Chromium reaches the network, renders, and then dies creating its first window.
Nothing in the fleet ever asked whether that host could own a window, because
nothing published the answer and nothing consumed it.

This is the consumer. It reads the canonical registry for the candidate hosts,
reads each candidate's capability object

    stado://<namespace>/host_capabilities/<registry-target-name>.json

through the same `/api/object` endpoint the host-health beacon publishes and the
capability publisher writes, and matches `--requires` against what was measured.

Two rules make the answer honest:

  - a capability satisfies a requirement only when it was measured `true`;
  - a measurement older than `--max-stale-seconds` satisfies nothing. An old yes
    is not a yes -- a laptop that had a console session yesterday is a laptop
    with the lid shut today, and placing a browser job on it costs a whole run.

Output is split by channel so the answer is machine-readable:

  - success: exactly one host name on stdout, exit 0;
  - refusal: one line per candidate on stderr naming the measurement that
    disqualified it, nothing on stdout, exit 1;
  - the registry or this host's object API declaration being unreadable: exit 2.

Runs unchanged locally and as a Stado helper on any host: it takes the store URL,
namespace, bearer and CA from that host's own `~/.config/stado/config.json`.
"""

import argparse
import datetime
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
FIRST = len(["first"])
SCHEMA = "wisent.host-capabilities.v1"
PREFIX = "host_capabilities"
HOME = pathlib.Path(os.path.expanduser("~"))
CONFIG = pathlib.Path(os.environ.get("STADO_CONFIG") or HOME / ".config" / "stado" / "config.json")
# The registry is reached through the stado binary rather than re-derived from
# storage, because `registry pull` is what resolves and validates the canonical
# document; a second reader of that object is a second thing that can drift.
STADO = pathlib.Path(os.environ.get("STADO_BIN") or HOME / ".stado" / "bin" / "stado")
HTTP_TIMEOUT = float(len("s" * 20))
EXIT_REFUSED = len(["refused"])
EXIT_UNREADABLE = len(["cannot", "ask"])


class Unreadable(Exception):
    """The question could not be asked at all, as opposed to answered `no`."""


def storage_settings():
    if not CONFIG.is_file():
        raise Unreadable(f"{CONFIG} does not exist, so this host cannot name its object API")
    try:
        settings = json.loads(CONFIG.read_text(encoding="utf-8")).get("storage", {}).get("stado", {})
    except (OSError, ValueError) as problem:
        raise Unreadable(f"{CONFIG} is not readable JSON: {problem}") from problem
    if not settings.get("url"):
        raise Unreadable(f"{CONFIG} declares no storage.stado.url to read objects through")
    return settings


def bearer(settings):
    """The object API is authenticated; an unauthenticated read is a 401, not a no."""
    token_file = settings.get("token_file")
    if not token_file:
        return NONE
    path = pathlib.Path(os.path.expanduser(token_file))
    if not path.is_file():
        return NONE
    return path.read_text(encoding="utf-8").strip() or NONE


def tls_context(settings, url):
    """Honour storage.stado.ca_file: the tailnet endpoint presents a private CA."""
    if not url.startswith("https://"):
        return NONE
    ca_file = settings.get("ca_file")
    if not ca_file:
        return NONE
    path = pathlib.Path(os.path.expanduser(ca_file))
    if not path.is_file():
        return NONE
    return ssl.create_default_context(cafile=str(path))


def capability_uri(settings, target):
    namespace = settings.get("namespace") or "probierz"
    return f"stado://{namespace}/{PREFIX}/{target}.json"


def read_object(settings, uri):
    """Return the object body, or raise Unreadable naming what the API answered."""
    base = settings["url"].rstrip("/")
    endpoint = f"{base}/api/object?uri={urllib.parse.quote(uri, safe='')}"
    request = urllib.request.Request(endpoint, method="GET")
    token = bearer(settings)
    if token:
        request.add_header("Authorization", f"Bearer {token}")
    try:
        with urllib.request.urlopen(
            request, timeout=HTTP_TIMEOUT, context=tls_context(settings, base)
        ) as response:
            return response.read()
    except urllib.error.HTTPError as problem:
        if problem.code == int("404"):
            raise Unreadable(
                f"no capability object at {uri}; nothing has measured this host"
            ) from problem
        if problem.code in (int("401"), int("403")):
            raise Unreadable(
                f"the object API refused the read of {uri} ({problem.code}); "
                "this host's bearer is not scoped to that prefix"
            ) from problem
        raise Unreadable(f"the object API answered {problem.code} for {uri}") from problem
    except (urllib.error.URLError, OSError, ssl.SSLError) as problem:
        raise Unreadable(f"{base} is unreachable from here: {problem}") from problem


def registry_targets():
    if not STADO.is_file():
        raise Unreadable(f"{STADO} is not installed here, so the candidate list cannot be read")
    proc = subprocess.run(
        [str(STADO), "registry", "pull"], capture_output=True, text=True, check=False
    )
    if proc.returncode != ZERO:
        raise Unreadable(f"stado registry pull failed: {(proc.stderr or proc.stdout).strip()}")
    try:
        document = json.loads(proc.stdout)
    except ValueError as problem:
        raise Unreadable(f"stado registry pull did not print a registry: {problem}") from problem
    names = [entry.get("name") for entry in document.get("targets", []) if entry.get("name")]
    if not names:
        raise Unreadable("the canonical registry declares no targets")
    return names


def parse_measured_at(text):
    if not isinstance(text, str) or not text:
        return NONE
    try:
        stamp = datetime.datetime.fromisoformat(text.replace("Z", "+00:00"))
    except ValueError:
        return NONE
    if stamp.tzinfo is NONE:
        stamp = stamp.replace(tzinfo=datetime.timezone.utc)
    return stamp


def disqualification(target, document, requires, max_stale_seconds, now):
    """Return the one sentence that disqualifies this host, or NONE if it holds."""
    if not isinstance(document, dict):
        return "its capability object is not a JSON object"
    schema = document.get("schema")
    if schema != SCHEMA:
        return f"its capability object declares schema {schema!r}, not {SCHEMA}"
    # An object naming a different host is the copy-paste failure this whole
    # model exists to catch: one machine's measurement answering for another.
    named = document.get("host")
    if named != target:
        return f"its capability object measures host {named!r}, not this registry target"

    measured_at = parse_measured_at(document.get("measured_at"))
    if measured_at is NONE:
        return f"its measured_at {document.get('measured_at')!r} is not a timestamp"
    age = (now - measured_at).total_seconds()
    if age > max_stale_seconds:
        return (
            f"its measurement is {age:.0f}s old, past the {max_stale_seconds:.0f}s window; "
            "an old yes is not a yes"
        )

    capabilities = document.get("capabilities")
    if not isinstance(capabilities, dict):
        return "its capability object publishes no capabilities map"
    for capability in requires:
        measurement = capabilities.get(capability)
        if not isinstance(measurement, dict):
            return f"it does not measure {capability} at all"
        if measurement.get("value") is not True:
            detail = measurement.get("detail") or "no detail recorded"
            return f"{capability} measured false: {detail}"
    return NONE


def place(requires, max_stale_seconds, candidates=NONE):
    """Return (host, refusals). `host` is NONE when no candidate qualifies.

    Importable so the installer asks the same question the operator does; a
    second copy of this matching is a second answer waiting to disagree.
    """
    settings = storage_settings()
    names = candidates or registry_targets()
    now = datetime.datetime.now(datetime.timezone.utc)
    qualified = []
    refusals = []
    for target in sorted(names):
        uri = capability_uri(settings, target)
        try:
            body = read_object(settings, uri)
            document = json.loads(body)
        except Unreadable as problem:
            refusals.append(f"{target}  {problem}")
            continue
        except ValueError as problem:
            refusals.append(f"{target}  its capability object is not readable JSON: {problem}")
            continue
        reason = disqualification(target, document, requires, max_stale_seconds, now)
        if reason:
            refusals.append(f"{target}  {reason}")
            continue
        qualified.append((parse_measured_at(document.get("measured_at")), target))

    if not qualified:
        return NONE, refusals
    # Several hosts can hold the same capability; take the freshest measurement,
    # then the first name, so one fleet state always answers with one host.
    qualified.sort(key=lambda item: (-item[ZERO].timestamp(), item[FIRST]))
    return qualified[ZERO][FIRST], refusals


def main():
    parser = argparse.ArgumentParser(
        description="Name the one fleet host whose published measurements satisfy every requirement."
    )
    parser.add_argument(
        "--requires",
        required=True,
        help="comma-separated capability ids, for example display,browser-render",
    )
    parser.add_argument(
        "--max-stale-seconds",
        required=True,
        type=float,
        help="a measurement older than this satisfies nothing",
    )
    parser.add_argument(
        "--candidates",
        default="",
        help="comma-separated registry target names to consider; default is every registry target",
    )
    arguments = parser.parse_args()

    requires = [item.strip() for item in arguments.requires.split(",") if item.strip()]
    if not requires:
        # Placement with no requirement is not a placement question, and
        # answering it with some host is how work lands where it cannot run.
        print("--requires named no capability; there is nothing to match on", file=sys.stderr)
        return EXIT_UNREADABLE

    candidates = [item.strip() for item in arguments.candidates.split(",") if item.strip()]
    try:
        host, refusals = place(requires, arguments.max_stale_seconds, candidates or NONE)
    except Unreadable as problem:
        print(str(problem), file=sys.stderr)
        return EXIT_UNREADABLE

    if host is NONE:
        for line in refusals:
            print(line, file=sys.stderr)
        return EXIT_REFUSED
    print(host)
    return ZERO


if __name__ == "__main__":
    sys.exit(main())
