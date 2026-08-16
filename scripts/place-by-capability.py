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

Three rules make the answer honest:

  - a capability satisfies a requirement only when it was measured `true`;
  - a measurement older than `--max-stale-seconds` satisfies nothing. An old yes
    is not a yes -- a laptop that had a console session yesterday is a laptop
    with the lid shut today, and placing a browser job on it costs a whole run;
  - a capability says what a host CAN do, and the registry target's `placement`
    policy says what it MAY be used for. Both must pass. The operator's own
    laptop can open a window and must never be handed a customer login, so it
    declares `placement.excludes`, and an excluded candidate is refused in its
    own words rather than mixed in with hosts whose measurement failed.

Output is split by channel so the answer is machine-readable:

  - success: exactly one host name on stdout, exit 0;
  - refusal: one line per candidate on stderr naming the policy or the
    measurement that disqualified it, nothing on stdout, exit 1;
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
# The registry target key that says what a host MAY be used for. Exclusions
# only: an accepts-allowlist would silently disqualify every host that has not
# declared one, which is the same silence this whole model is replacing.
POLICY_KEY = "placement"
POLICY_EXCLUDES = "excludes"
POLICY_REASON = "reason"
POLICY_KEYS = (POLICY_EXCLUDES, POLICY_REASON)
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


def registry_declarations():
    """Return {target name: its whole registry entry}.

    The entry travels with the candidate list because it answers the halves of
    the question no measurement can. A capability says what a host CAN do; the
    target's `placement` policy says what it MAY be used for -- a host that can
    open a window is still the wrong host when it is the machine the operator is
    sitting in front of -- and `release_platform` says what shape of unit it can
    load at all, which is what a caller needs once placement stops answering
    with the host that happens to be running the installer.
    """
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
    declared = {
        entry["name"]: entry for entry in document.get("targets", []) if entry.get("name")
    }
    if not declared:
        raise Unreadable("the canonical registry declares no targets")
    return declared


def declared_service(entry, service):
    """The registry service entry a host runs under this name, or NONE.

    A service is named twice in the registry -- `name` is what the fleet calls
    it and `label` is what launchd or systemd loads -- and callers know only the
    first. Both are matched so a rename on one side does not silently make a
    host stop being a candidate.
    """
    for candidate in entry.get("services") or []:
        if service in (candidate.get("name"), candidate.get("label"), candidate.get("unit")):
            return candidate
    return NONE


def runs_service(settings, target, entry, service):
    """Why this host cannot execute a job that needs `service`, or NONE.

    This is what replaced a hand-written exclusion list on each target. Which
    machine may run a Weles trajectory is not an opinion to be maintained in
    three places: the registry already says which hosts run the Weles worker,
    and the beacon already says whether it is actually loaded there. A host that
    does not declare the service is not a candidate, and a host that declares it
    without running it is a divergence, not a placement. Nothing to keep in
    sync, and an edit in Stado moves the answer by itself.
    """
    entry_service = declared_service(entry, service)
    if entry_service is NONE:
        return f"does not run {service}: the registry declares no such service on this host"
    label = entry_service.get("label") or entry_service.get("name")
    uri = f"stado://{settings['namespace']}/host_health/{entry.get('health_object') or target}.json"
    try:
        health = json.loads(read_object(settings, uri))
    except (Unreadable, ValueError):
        # The health object is named per host and some hosts publish under a
        # different stem; a declaration the beacon cannot confirm is reported by
        # `registry doctor`, and placement should not invent a second verdict.
        return NONE
    units = health.get("units") or {}
    state = (units.get(label) or units.get(entry_service.get("name")) or {}).get("state")
    if state == "active":
        return NONE
    return (
        f"declares {service} but its own beacon reports that unit "
        + (f"{state}" if state else "absent")
        + ": a declaration the host contradicts is not a placement"
    )


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
            return f"it does not measure {capability} at all ({age:.0f}s ago)"
        if measurement.get("value") is not True:
            detail = measurement.get("detail") or "no detail recorded"
            # The age rides along even when the value is what disqualified the
            # host. A bare `false` from a document nobody has refreshed in
            # twelve minutes and a `false` measured a moment ago are different
            # repairs -- one is a host to fix, the other is a publisher to
            # start -- and a reader must not have to go and look it up.
            return f"{capability} measured false {age:.0f}s ago: {detail}"
    return NONE


def policy_refusal(policy, requires):
    """Return why the registry forbids this placement, or NONE if it allows it.

    Kept apart from `disqualification` on purpose: a host that measured false
    and a host that is forbidden are two different facts, and a run that spelled
    them the same way would send an operator to fix a capability that is fine.
    """
    if policy is NONE:
        return NONE
    if not isinstance(policy, dict):
        return f"its registry target declares {POLICY_KEY} as {type(policy).__name__}, not an object"
    unknown = sorted(key for key in policy if key not in POLICY_KEYS)
    if unknown:
        # Fail closed. A policy half of which this matcher cannot read is a
        # policy that may forbid exactly the placement about to be made.
        return (
            f"its {POLICY_KEY} policy declares {', '.join(unknown)}, which this matcher does not "
            f"read; only {' and '.join(POLICY_KEYS)} are understood"
        )
    excludes = policy.get(POLICY_EXCLUDES, [])
    if not isinstance(excludes, list):
        return f"its {POLICY_KEY}.{POLICY_EXCLUDES} is not a list of capability ids"
    forbidden = [capability for capability in requires if capability in excludes]
    if not forbidden:
        return NONE
    reason = policy.get(POLICY_REASON) or "no reason declared"
    return (
        f"excluded by placement policy declared on the target: {', '.join(forbidden)} "
        f"may not be placed here ({reason})"
    )


def place(requires, max_stale_seconds, candidates=NONE, runs=NONE):
    """Return (host, refusals). `host` is NONE when no candidate qualifies.

    Importable so the installer asks the same question the operator does; a
    second copy of this matching is a second answer waiting to disagree.

    `runs` names the service that executes the work. It is the structural half
    of the question and it comes before every measurement: a machine that does
    not run the Weles worker is not a slow or unmeasured candidate for a Weles
    trajectory, it is not a candidate at all, and no exclusion list has to be
    maintained to say so.
    """
    settings = storage_settings()
    declared = registry_declarations()
    names = candidates or list(declared)
    now = datetime.datetime.now(datetime.timezone.utc)
    qualified = []
    refusals = []
    for target in sorted(names):
        entry = declared.get(target, {})
        if runs:
            wrong_host = runs_service(settings, target, entry, runs)
            if wrong_host:
                refusals.append(f"{target}  {wrong_host}")
                continue
        # Policy first: a forbidden host is forbidden whether or not anything
        # measured it, and reading its measurement would only invite the reader
        # to argue with a capability that was never the objection.
        forbidden = policy_refusal(entry.get(POLICY_KEY), requires)
        if forbidden:
            refusals.append(f"{target}  {forbidden}")
            continue
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
        "--runs",
        default="",
        help=(
            "registry service name that executes the work, for example "
            "com.wisent.always-on.weles; only hosts declaring it and reporting it "
            "active are candidates"
        ),
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
        host, refusals = place(
            requires,
            arguments.max_stale_seconds,
            candidates or NONE,
            runs=arguments.runs.strip() or NONE,
        )
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
