#!/usr/bin/env python3
"""Report registry declarations that no reader honours, or that the world contradicts.

This is the runnable half of the three findings added to `stado registry doctor`
(`self-referencing-endpoint`, `capability-unsatisfied`, `unread-declaration`). The
Rust command is the permanent home; this exists because nothing on the fleet
rebuilds stado on a push -- `deploy.yml` builds only on `stado-v*` tags -- so a
check that lives only in Rust would not run anywhere for weeks, which is exactly
how the declarations below came to sit unread in the first place.

The three shapes, all of them found on this fleet on 2026-08-14:

  self-referencing-endpoint  A service directory endpoint for a host equals a
                             resolver socket on that same host, so the adapter
                             whose whole job is to forward that port dials
                             itself. This is what took every `stado host ...`
                             command down when the resolver read its storage
                             through its own adapter.
  capability-unsatisfied     A declared unit needs something of the host
                             (`display`, `browser-render`) that the last
                             measurement says the host does not have, or that
                             nothing has measured recently enough to say.
  unread-declaration         A field an operator wrote that no code path reads.
                             `weles.actions` and `storage.stado.ca_file` are the
                             two known cases; the rule is general, so a field
                             added tomorrow with no reader fails here too.

Nothing here is hardcoded to those cases. The rules are derived from the Rust
sources that own them -- the declaration catalog in `capabilities.rs`, the
`ComputeTarget` model in `targets.rs`, the liveness window in `constants.rs` -- and
the script fails if its own copy of the catalog has drifted from theirs, because a
checker that disagrees with the code it is checking is worse than no checker.

Read-only: it pulls the registry, reads the local configuration, and GETs the
capability objects. It writes nothing and changes no host.
"""

import argparse
import datetime
import http
import json
import os
import pathlib
import re
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request

NONE = None
ZERO = len([])
EXIT_FINDINGS = len("a")
HOME = pathlib.Path(os.path.expanduser("~"))
REPO = pathlib.Path(__file__).resolve().parent.parent
CAPABILITIES_RS = REPO / "stado-rs" / "src" / "capabilities.rs"
TARGETS_RS = REPO / "stado-rs" / "src" / "targets.rs"
CONSTANTS_RS = REPO / "stado-rs" / "src" / "constants.rs"
REGISTRY_RS = REPO / "stado-rs" / "src" / "cli" / "registry.rs"
STADO = HOME / ".stado" / "bin" / "stado"
CONFIG = pathlib.Path(
    os.environ.get("STADO_CONFIG_FILE") or HOME / ".config" / "stado" / "config.json"
)
# Object prefix the measurements live under, alongside `host_health/` and read
# through the same object API.
CAPABILITIES_PREFIX = "host_capabilities"
CAPABILITIES_SCHEMA = "wisent.host-capabilities.v1"
# The job author's own declaration, published verbatim so the registry never has
# to carry a second copy of a capability list.
REQUIREMENTS_PREFIX = "job_requirements"
REQUIREMENTS_SCHEMA = "wisent.trajectory-requirements.v1"
# Long enough for a tunnelled object read, short enough that an unreachable
# adapter reports rather than hangs a cron.
HTTP_TIMEOUT = len("a" * 10)
NOT_FOUND = http.HTTPStatus.NOT_FOUND
REGISTRY_TARGET = "registry target"
CONFIGURATION = "config"

# Mirror of `capabilities::DECLARED_FIELDS`. That catalog is the authority; this
# copy exists so the check runs without a compiler, and `catalog_paths()` below
# refuses to run if the two have diverged.
CATALOG = [
    {
        "surface": REGISTRY_TARGET,
        "path": "services",
        "consumer": "fleet",
        "reader": "cli::registry::declared_units, deploy::service, cli::service",
        "when": NONE,
    },
    {
        "surface": REGISTRY_TARGET,
        "path": "service_resolver",
        "consumer": "fleet",
        "reader": "service_resolution::resolver_config",
        "when": NONE,
    },
    {
        "surface": REGISTRY_TARGET,
        "path": "gpu_power_limit_watts",
        "consumer": "fleet",
        "reader": "providers::local::agent::reconcile_gpu_power_limit",
        "when": NONE,
    },
    {
        # An excluded capability is a policy answer ("may not run here") and not a
        # measurement, so the matcher that reads this reports it as its own kind of
        # refusal. The reader is Python today; a reader is named, not typed.
        "surface": REGISTRY_TARGET,
        "path": "placement",
        "consumer": "fleet",
        "reader": "scripts/place-by-capability.py",
        "when": NONE,
    },
    {
        # Read by another repository's binary (`transcript-label-trainer`), which
        # is as much a reader as a script is. Uncatalogued, both keys read as
        # unread while a trainer honoured them.
        "surface": REGISTRY_TARGET,
        "path": "training",
        "consumer": "fleet",
        "reader": "transcript-label-trainer placement::declared_training",
        "when": NONE,
    },
    {
        "surface": REGISTRY_TARGET,
        "path": "transcript_lake",
        "consumer": "fleet",
        "reader": "transcript-label-trainer placement::declared_lake_root",
        "when": NONE,
    },
    {
        "surface": REGISTRY_TARGET,
        "path": "weles.actions",
        "consumer": "operator-copy",
        "command": "stado placement weles-policy publish",
        "destination": "~/.config/weles/placement-policy.json on the target host",
        "when": NONE,
    },
    {
        "surface": CONFIGURATION,
        "path": "storage.stado.ca_file",
        "consumer": "fleet",
        "reader": "queue::stado_object::StadoObjectBackend::client",
        "when": {"path": "storage.stado.url", "prefix": "https://"},
    },
]


def rust_text(path):
    return path.read_text(encoding="utf-8")


def catalog_paths():
    """The dotted paths `capabilities.rs` catalogues, and whether we agree.

    Two independent lists of "which declaration has a reader" would drift within
    a week, and the drift would show up as a clean run rather than as an error,
    so this compares them on every invocation and refuses to continue.
    """
    text = rust_text(CAPABILITIES_RS)
    block = text.split("pub const DECLARED_FIELDS", len("aa"))[len("a")]
    block = block.split("\n];", len("aa"))[ZERO]
    found = set(re.findall(r'DeclaredField::\w+\(\s*"([^"]+)"', block))
    ours = {entry["path"] for entry in CATALOG}
    if found != ours:
        missing = ", ".join(sorted(found - ours)) or "(none)"
        extra = ", ".join(sorted(ours - found)) or "(none)"
        raise SystemExit(
            f"catalog drift: {CAPABILITIES_RS.name} has {missing} that this script does not, "
            f"and this script has {extra} that it does not"
        )
    return found


def modelled_target_fields():
    """Field names `ComputeTarget` models, so a key it does not model is derived
    rather than listed.

    That set is the whole derived half of the unread-declaration rule: a registry
    key the model does not name lands in `ComputeTarget::extra`, which is by
    construction the set of keys no typed reader in the binary can consult.
    """
    text = rust_text(TARGETS_RS)
    block = text.split("pub struct ComputeTarget {", len("aa"))[len("a")]
    block = block.split("\n}", len("aa"))[ZERO]
    return set(re.findall(r"^\s*pub (\w+):", block, re.MULTILINE)) - {"extra"}


def liveness_seconds():
    """The one window both liveness signals use, read where it is defined."""
    text = rust_text(CONSTANTS_RS)
    found = re.search(r"CAPACITY_STALE_SECONDS: u64 = (\d+)", text)
    if not found:
        raise SystemExit(f"no CAPACITY_STALE_SECONDS in {CONSTANTS_RS}")
    return int(found.group(len("a")))


def republication_seconds():
    """How long a published requirement declaration is believed, from the Rust.

    A requirement is republished when the job changes rather than on a heartbeat,
    so it gets its own much longer window; taking the number from
    `registry.rs::requirements_stale_after_seconds` keeps this script and the
    command it stands in for judging the same object the same way.
    """
    text = rust_text(REGISTRY_RS)
    found = re.search(r"fn requirements_stale_after_seconds\(\)[^{]*\{\s*TimeDelta::(\w+)\((\d+)\)", text)
    if not found:
        raise SystemExit(f"no requirements_stale_after_seconds in {REGISTRY_RS}")
    unit, amount = found.group(len("a")), int(found.group(len("aa")))
    seconds = {
        "seconds": len("a"),
        "minutes": len("a" * 60),
        "hours": len("a" * 3600),
        "days": len("a" * 86400),
        "weeks": len("a" * 604800),
    }.get(unit)
    if seconds is NONE:
        raise SystemExit(f"unsupported TimeDelta unit {unit} in {REGISTRY_RS}")
    return amount * seconds


def read_registry(source):
    """The canonical document, or the one an operator is about to publish.

    A staged file is worth auditing before it is pushed: the endpoint that
    proxies to itself was published once already, and a document that never
    reaches the store cannot break anything.
    """
    if source == "-":
        return json.loads(sys.stdin.read()), "stdin"
    if source:
        path = pathlib.Path(os.path.expanduser(source))
        return json.loads(path.read_text(encoding="utf-8")), str(path)
    proc = subprocess.run(
        [str(STADO), "registry", "pull"],
        capture_output=True,
        text=True,
        check=False,
        cwd=str(HOME),
    )
    if proc.returncode != ZERO:
        raise SystemExit(f"stado registry pull failed: {proc.stderr.strip()}")
    return json.loads(proc.stdout), "stado registry pull"


def config_document():
    if not CONFIG.is_file():
        return {}
    return json.loads(CONFIG.read_text(encoding="utf-8"))


def value_at(root, dotted):
    """The value at a dotted path, or None when any segment is absent."""
    current = root
    for key in dotted.split("."):
        if not isinstance(current, dict) or key not in current:
            return NONE
        current = current[key]
    return current


def rendered(value):
    """One line of JSON, so a finding shows what was written and not only where."""
    return json.dumps(value, sort_keys=True)


def socket_of(url):
    """`http://127.0.0.1:17614` and `127.0.0.1:17614` as one comparable pair."""
    parsed = urllib.parse.urlparse(url)
    if not parsed.hostname or not parsed.port:
        return NONE
    return f"{parsed.hostname}:{parsed.port}"


def resolver_sockets(target):
    """Every socket this host's resolver publishes, and what it serves there."""
    resolver = target.get("service_resolver") or {}
    sockets = {}
    api = resolver.get("api_bind")
    if api:
        sockets[api] = "the resolver's own API"
    for adapter in resolver.get("adapters") or []:
        if adapter.get("bind"):
            sockets[adapter["bind"]] = f"adapter for {adapter.get('service')}"
    return sockets


def self_referencing_endpoints(document):
    findings = []
    services = ((document.get("service_directory") or {}).get("services")) or {}
    for target in document.get("targets") or []:
        name = target.get("name")
        sockets = resolver_sockets(target)
        for service, route in services.items():
            for key in ("endpoints", "standby"):
                endpoint = (route.get(key) or {}).get(name)
                if not endpoint:
                    continue
                socket = socket_of(endpoint.get("url", ""))
                if socket in sockets:
                    findings.append(
                        (
                            "self-referencing-endpoint",
                            name,
                            f"service_directory.services.{service}.{key}.{name} is {socket}, "
                            f"which is {sockets[socket]} on that same target: the adapter would "
                            "proxy to itself",
                        )
                    )
    return findings


def declared_trajectories(document):
    """Which job each declared service runs, by trajectory id.

    The registry's whole role in this join is the identifier: `trajectory` on the
    service entry, never a capability list. The list belongs to the job's author
    and is published as an object, because a copy of it here would be the second
    source of truth this command exists to report. A service that names no
    trajectory declares no requirement.
    """
    claims = []
    for target in document.get("targets") or []:
        name = target.get("name")
        for entry in target.get("services") or []:
            trajectory = entry.get("trajectory")
            if not isinstance(trajectory, str) or not trajectory:
                continue
            unit = entry.get("name") or entry.get("label") or "(unnamed service)"
            claims.append((name, unit, trajectory))
    return claims


def object_api(config):
    """A reader and a lister for this deployment's object API.

    Same API and same bearer the host-health beacon is published and read through,
    so the audit sees exactly what the fleet sees rather than a second opinion.
    """
    stado = (config.get("storage") or {}).get("stado") or {}
    base = (stado.get("url") or "").rstrip("/")
    namespace = stado.get("namespace") or ""
    token_file = stado.get("token_file") or ""
    token = ""
    if token_file:
        path = pathlib.Path(os.path.expanduser(token_file))
        token = path.read_text(encoding="utf-8").strip() if path.is_file() else ""

    def fetch(url):
        request = urllib.request.Request(url)
        if token:
            request.add_header("Authorization", f"Bearer {token}")
        try:
            with urllib.request.urlopen(request, timeout=HTTP_TIMEOUT) as response:
                return json.loads(response.read()), NONE
        except urllib.error.HTTPError as error:
            # An absent object is "nothing has measured this host" and earns a
            # finding of its own; any other status is a read failure, a different
            # fact that must not be reported as an unmeasured host.
            if error.code == NOT_FOUND:
                return NONE, NONE
            return NONE, f"{url} answered HTTP {error.code}"
        except (OSError, ValueError) as error:
            # OSError, not URLError: a reset mid-response (`ConnectionResetError`
            # from the object API's own socket) escapes urllib unwrapped, and it
            # took this whole audit down with a traceback while every finding it
            # had already computed was thrown away. A read that failed is a fact
            # this command reports, never a crash.
            return NONE, f"{url} could not be read: {error}"

    def read(key):
        uri = f"stado://{namespace}/{key}"
        return fetch(f"{base}/api/object?uri={urllib.parse.quote(uri, safe='')}")

    def listing(prefix):
        query = urllib.parse.urlencode({"namespace": namespace, "prefix": prefix})
        body, error = fetch(f"{base}/api/object/list?{query}")
        if error:
            return NONE, error
        return (body or {}).get("objects") or [], NONE

    return read, listing, f"{base or '(no storage.stado.url)'} namespace {namespace or '(none)'}"


def job_requirements(listing, read, window, now):
    """Every published requirement declaration, resolved to what each job needs.

    `stado://<namespace>/job_requirements/weles-trajectories.json` carries the
    bytes of `weles/scripts/trajectories/requirements.json`: the job's author
    publishes it, and this reads it. Objects that carry an unknown schema or that
    nobody has republished inside `window` are refused by name rather than
    ignored, because a service pointing into one must not pass as satisfied.
    """
    needs = {}
    objects = []
    refused = []
    entries, error = listing(f"{REQUIREMENTS_PREFIX}/")
    if error:
        return needs, objects, refused, error
    for entry in entries:
        key = entry.get("key") or ""
        if not key.endswith(".json"):
            continue
        objects.append(key)
        body, read_error = read(key)
        if read_error or body is NONE:
            refused.append(f"{key} is not readable: {read_error or 'absent'}")
            continue
        if body.get("schema") != REQUIREMENTS_SCHEMA:
            refused.append(
                f"{key} carries schema {body.get('schema')!r} rather than {REQUIREMENTS_SCHEMA}"
            )
            continue
        published = entry.get("updated_at")
        stamp = NONE
        if isinstance(published, str):
            try:
                stamp = datetime.datetime.fromisoformat(published.replace("Z", "+00:00"))
            except ValueError:
                stamp = NONE
        if stamp is not NONE and (now - stamp).total_seconds() > window:
            refused.append(
                f"{key} was published {human_age(now - stamp)} ago ({stamp.isoformat()}), past "
                f"the {window}s republication window"
            )
            continue
        for trajectory, value in (body.get("trajectories") or {}).items():
            needs[trajectory] = (
                key,
                [item for item in (value or []) if isinstance(item, str)],
            )
    return needs, objects, refused, NONE


def consulted(objects, refused):
    """What was read, for a finding that has to explain an absence."""
    read = (
        f"read {', '.join(objects)}"
        if objects
        else f"no object exists under {REQUIREMENTS_PREFIX}/"
    )
    return f"{read}; refused {'; '.join(refused)}" if refused else read


def measured_at(body):
    stamp = (body or {}).get("measured_at")
    if not isinstance(stamp, str):
        return NONE
    try:
        return datetime.datetime.fromisoformat(stamp.replace("Z", "+00:00"))
    except ValueError:
        return NONE


def human_age(delta):
    """Largest whole unit, the same spelling `registry doctor` prints."""
    seconds = int(delta.total_seconds())
    for amount, suffix in (
        (seconds // (len("a" * 86400)), "d"),
        (seconds // len("a" * 3600), "h"),
        (seconds // len("a" * 60), "m"),
    ):
        if amount > ZERO:
            return f"{amount}{suffix}"
    return f"{max(seconds, ZERO)}s"


def measurement_gaps(host, listed, body, error, window, now):
    """What stops one host's last measurement from satisfying `listed`.

    One clause per reason, empty when nothing stops it. The single place that
    answers "can this host run this job", so the finding about a host that declares
    the job and the finding about a job no host declares cannot answer it
    differently.
    """
    if not listed:
        return []
    if error:
        return [f"{CAPABILITIES_PREFIX}/{host}.json could not be read: {error}"]
    if body is NONE:
        return [
            f"{CAPABILITIES_PREFIX}/{host}.json does not exist: nothing has measured this host"
        ]
    if body.get("schema") != CAPABILITIES_SCHEMA:
        return [
            f"{CAPABILITIES_PREFIX}/{host}.json carries schema {body.get('schema')!r} rather "
            f"than {CAPABILITIES_SCHEMA}"
        ]
    stamp = measured_at(body)
    if stamp is NONE:
        return [
            f"{CAPABILITIES_PREFIX}/{host}.json carries no readable measured_at, so its age "
            "cannot be judged"
        ]
    age = now - stamp
    if age.total_seconds() > window:
        return [
            f"{CAPABILITIES_PREFIX}/{host}.json was measured {human_age(age)} ago "
            f"({stamp.isoformat()}), past the {window}s liveness window"
        ]
    gaps = []
    measured = body.get("capabilities") or {}
    for capability in listed:
        entry = measured.get(capability)
        if entry is NONE:
            gaps.append(f"{CAPABILITIES_PREFIX}/{host}.json does not measure {capability}")
        elif entry.get("value") is not True:
            gaps.append(
                f"{capability} measured false: {entry.get('detail') or 'no detail recorded'}"
            )
    return gaps


def capability_findings(document, read, needs, objects, refused, window, now):
    """Every hop of the join that fails: service -> trajectory -> requirement ->
    measurement.

    One line for a missing declaration or a missing, mis-schema'd or stale
    measurement, and one line per capability once both documents are usable: the
    operator's next action is the same however many ids were named.
    """
    findings = []
    measurements = {}
    for host, unit, trajectory in declared_trajectories(document):
        if trajectory not in needs:
            findings.append(
                (
                    "capability-unsatisfied",
                    host,
                    f"{unit} runs trajectory {trajectory}, and no published declaration names "
                    f"it: {consulted(objects, refused)}",
                )
            )
            continue
        source, listed = needs[trajectory]
        if host not in measurements:
            measurements[host] = read(f"{CAPABILITIES_PREFIX}/{host}.json")
        body, error = measurements[host]
        preamble = f"{unit} runs {trajectory}, which {source} says requires {', '.join(listed)},"
        for gap in measurement_gaps(host, listed, body, error, window, now):
            findings.append(("capability-unsatisfied", host, f"{preamble} and {gap}"))
    return findings, measurements


def unplaced_jobs(document, read, needs, measurements, window, now):
    """Jobs in the published roster that nothing runs and no host could.

    A declared service entry is the RESULT of a placement that succeeded, so a
    trajectory no target declares is a job waiting for a host. That is worth
    reporting only when no candidate can take it: while some measured host
    satisfies the requirement, placement has an answer and the absence is a step
    not yet taken rather than a contradiction. The row names every candidate and
    the measurement that disqualified it, so it says what would have to change.
    """
    findings = []
    placed = {trajectory for _, _, trajectory in declared_trajectories(document)}
    hosts = [
        target.get("name")
        for target in document.get("targets") or []
        # Only kind=local names a machine that can hold a session and run a
        # browser; a "gcp" or "vast" target is a dispatcher pool.
        if target.get("kind") == "local"
    ]
    for trajectory, (source, listed) in sorted(needs.items()):
        if not listed or trajectory in placed:
            continue
        disqualified = []
        for host in hosts:
            if host not in measurements:
                measurements[host] = read(f"{CAPABILITIES_PREFIX}/{host}.json")
            body, error = measurements[host]
            gaps = measurement_gaps(host, listed, body, error, window, now)
            if not gaps:
                disqualified = []
                break
            disqualified.append(f"{host}: {', '.join(gaps)}")
        if disqualified:
            findings.append(
                (
                    "job-unplaced",
                    trajectory,
                    f"{source} says it requires {', '.join(listed)}, no registry target declares "
                    f"a service that runs it, and no host can take it — "
                    f"{'; '.join(disqualified)}",
                )
            )
    return findings


def unread_reason(entry, sibling):
    """Why a declared value never reaches behaviour, or None when it does."""
    if entry["consumer"] == "fleet":
        condition = entry["when"]
        if condition is NONE:
            return NONE
        observed = sibling if sibling is not NONE else "(absent)"
        if str(observed).startswith(condition["prefix"]):
            return NONE
        return (
            f"its only reader {entry['reader']} runs when {condition['path']} starts with "
            f"{condition['prefix']!r}, and that key is {observed}"
        )
    if entry["consumer"] == "operator-copy":
        return (
            f"no fleet process reads it: only `{entry['command']}` copies it to "
            f"{entry['destination']}, and only when an operator runs that command"
        )
    return "no code path in this build reads it"


def sibling_value(root, entry):
    condition = entry["when"]
    if condition is NONE:
        return NONE
    value = value_at(root, condition["path"])
    if value is NONE:
        return NONE
    return value if isinstance(value, str) else rendered(value)


def catalogued(surface, path):
    for entry in CATALOG:
        if entry["surface"] == surface and entry["path"] == path:
            return entry
    return NONE


def unread_declarations(document, config, modelled):
    findings = []
    for target in document.get("targets") or []:
        name = target.get("name")
        for key, value in sorted(target.items()):
            if key in modelled:
                continue
            entry = catalogued(REGISTRY_TARGET, key)
            if entry is NONE:
                findings.append(
                    (
                        "unread-declaration",
                        name,
                        f"registry target key {key} is declared as {rendered(value)} and is "
                        "neither modelled by ComputeTarget nor catalogued in "
                        "capabilities::DECLARED_FIELDS, so no reader in this build can consult it",
                    )
                )
                continue
            reason = unread_reason(entry, sibling_value(target, entry))
            if reason:
                findings.append(
                    (
                        "unread-declaration",
                        name,
                        f"{REGISTRY_TARGET} {key} is declared as {rendered(value)} but {reason}",
                    )
                )
        # Dotted paths only: a top-level catalogued key was answered above, and
        # answering it twice would report one defect as two.
        for entry in CATALOG:
            if entry["surface"] != REGISTRY_TARGET or "." not in entry["path"]:
                continue
            value = value_at(target, entry["path"])
            if value is NONE:
                continue
            reason = unread_reason(entry, sibling_value(target, entry))
            if reason:
                findings.append(
                    (
                        "unread-declaration",
                        name,
                        f"{REGISTRY_TARGET} {entry['path']} is declared as {rendered(value)} but "
                        f"{reason}",
                    )
                )
    for entry in CATALOG:
        if entry["surface"] != CONFIGURATION:
            continue
        value = value_at(config, entry["path"])
        if value is NONE:
            continue
        reason = unread_reason(entry, sibling_value(config, entry))
        if reason:
            findings.append(
                (
                    "unread-declaration",
                    str(CONFIG),
                    f"{CONFIGURATION} {entry['path']} is declared as {rendered(value)} but "
                    f"{reason}",
                )
            )
    return findings


def print_table(rows):
    headers = ("FINDING", "SUBJECT", "DETAIL")
    widths = [
        max(len(str(row[column])) for row in (rows + [headers]))
        for column in range(len(headers))
    ]
    print()
    print("  ".join(header.ljust(widths[index]) for index, header in enumerate(headers)).rstrip())
    for row in rows:
        print(
            "  ".join(
                str(cell).ljust(widths[index]) if index < len(headers) - len("a") else str(cell)
                for index, cell in enumerate(row)
            ).rstrip()
        )


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[ZERO])
    parser.add_argument(
        "--registry",
        default=NONE,
        help="document to audit: a path, '-' for stdin, or omitted for `stado registry pull`",
    )
    parser.add_argument("--json", action="store_true", help="emit the findings as JSON")
    args = parser.parse_args()

    catalogue = catalog_paths()
    modelled = modelled_target_fields()
    window = liveness_seconds()
    republication = republication_seconds()
    document, source = read_registry(args.registry)
    config = config_document()
    read, listing, endpoint = object_api(config)
    now = datetime.datetime.now(datetime.timezone.utc)

    claims = declared_trajectories(document)
    # The roster is read whether or not anything declares a trajectory, because a
    # job the roster names and no target declares is itself a finding: a declared
    # service entry is the result of a placement that succeeded.
    needs, objects, refused, listing_error = job_requirements(listing, read, republication, now)
    if listing_error:
        refused = refused + [f"{REQUIREMENTS_PREFIX}/ could not be listed: {listing_error}"]

    findings = self_referencing_endpoints(document)
    capabilities, measurements = capability_findings(
        document, read, needs, objects, refused, window, now
    )
    findings += capabilities
    findings += unplaced_jobs(document, read, needs, measurements, window, now)
    findings += unread_declarations(document, config, modelled)
    measured = len([host for host, (body, _) in measurements.items() if body])

    generation = (document.get("service_directory") or {}).get("generation")
    if not args.json:
        print(f"registry      {source} (service_directory generation {generation})")
        print(f"targets       {len(document.get('targets') or [])}")
        print(f"objects       {endpoint}")
        print(
            f"claims        {len(claims)} service(s) naming a trajectory, "
            f"{len(needs)} declared trajectory requirement(s), {measured} host(s) measured"
        )
        print(f"liveness      {window}s from {CONSTANTS_RS.name} CAPACITY_STALE_SECONDS")
        print(
            f"republication {republication}s from {REGISTRY_RS.name} "
            "requirements_stale_after_seconds"
        )
        print(f"catalog       {len(catalogue)} declaration(s) from {CAPABILITIES_RS.name}")
        print(f"modelled      {len(modelled)} ComputeTarget field(s) from {TARGETS_RS.name}")
    if args.json:
        print(
            json.dumps(
                {
                    "registry": source,
                    "ok": not findings,
                    "checked": {
                        "targets": len(document.get("targets") or []),
                        "requirement_claims": len(claims),
                        "declared_trajectories": len(needs),
                        "capability_measurements": measured,
                    },
                    "findings": [
                        {"finding": kind, "subject": subject, "detail": detail}
                        for kind, subject, detail in findings
                    ],
                },
                indent=len("aa"),
                sort_keys=True,
            )
        )
    elif findings:
        print_table(findings)
    else:
        print("\nevery declaration in this document has a reader and a measurement that agrees")
    return EXIT_FINDINGS if findings else ZERO


sys.exit(main())
