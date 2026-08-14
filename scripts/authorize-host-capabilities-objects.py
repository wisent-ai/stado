#!/usr/bin/env python3
"""Let this host's object API carry the fleet's declaration documents.

`stado://probierz/host_capabilities/<host>.json` answered 401 for every caller
while `host_health/<host>.json` and `registry.json` answered 200, and the bearer
was never even compared: `dashboard::authorize_object` looks the key up in
`config::object_api_namespaces()` first and refuses a key no prefix policy
covers (stado-rs/src/dashboard/mod.rs:2607-2641). A capability nobody can read
is a capability nobody will declare, so the prefix has to exist before the
measurement is worth publishing.

Two prefixes are granted, because the fleet's new model has two halves and they
are useless apart: `host_capabilities/` is what each host measures about itself,
`job_requirements/` is what a job declares it needs, and placement, the runtime
guard and `registry doctor` all read one against the other. Both are granted by
copying the policy that already governs `host_health/` -- the same item, the
same verbs, nothing wider. All three are the same kind of thing: a small
document one party publishes and the rest of the fleet reads.

Idempotent. It prints the prefix list before and after and never prints a
credential; the namespace entry names its Skarbiec item, and the item's fields
are never read here.

The object API resolves this map once per process
(stado-rs/src/config.rs:1512-1529 is a `LazyLock`), so a change here reaches
callers only after that unit restarts. This script deliberately does not
restart it: the same process serves the canonical registry to the whole fleet,
and that restart belongs to an operator watching the fleet, not to a config
edit.
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
NAMESPACE = "probierz"
SERVICE = "stado-object-api"
GRANTED = ("host_capabilities/", "job_requirements/")
MODEL = "host_health/"
KEY = f"object_api.namespaces.{NAMESPACE}"


def run(*args, env=NONE):
    proc = subprocess.run(
        args,
        capture_output=True,
        text=True,
        check=False,
        env=env,
        timeout=len("a" * 60),
    )
    return proc.returncode, (proc.stdout + proc.stderr).strip()


def config_path():
    """The config file the object API on this host actually reads.

    A unit that carries `STADO_CONFIG` reads that file and not the account
    default, and editing the wrong one produces a change that validates, commits
    and does nothing -- exactly the failure this whole pass exists to stop.
    """
    named = os.environ.get("STADO_CONFIG", "").strip()
    if named:
        return pathlib.Path(named).expanduser()
    return HOME / ".config" / "stado" / "config.json"


def declared_port():
    """The port this host publishes the object API on, per the registry.

    Hard-coding the number would be a second declaration of the same fact, and
    the fleet already has one: the service directory. A host with no endpoint of
    its own serves no object API, and there is then no process to inspect.
    """
    code, output = run(str(STADO), "registry", "pull")
    if code != ZERO:
        return NONE
    document = json.loads(output)
    service = document["service_directory"]["services"].get(SERVICE, {})
    node = os.uname().nodename.lower().split(".")[ZERO]
    for name, endpoint in service.get("endpoints", {}).items():
        target = next(
            (entry for entry in document["targets"] if entry.get("name") == name), {}
        )
        names = [str(value).lower() for value in target.get("hostnames", [])] + [name.lower()]
        if any(value.split(".")[ZERO] == node for value in names if value):
            return str(endpoint.get("url", "")).rsplit(":", len("a"))[-1]
    return NONE


def serving_process_environment(port):
    """The environment of the local object-API process, keys only.

    `WC_OBJECT_API_NAMESPACES` beats the config file when it is set
    (stado-rs/src/config.rs:1514-1526). If the live process carries it, a file
    edit is a no-op and this script must say so rather than report success.
    """
    if port is NONE:
        return []
    code, listeners = run("/usr/sbin/lsof", "-nP", f"-iTCP:{port}", "-sTCP:LISTEN")
    pids = sorted(
        {line.split()[len("a")] for line in listeners.splitlines()[len("a"):] if line.split()}
    )
    findings = []
    for pid in pids:
        code, text = run("/bin/ps", "-ww", "-E", "-o", "command=", "-p", pid)
        if code != ZERO:
            continue
        program = text.split(" WC_")[ZERO].split(" STADO_")[ZERO]
        carries_env = " WC_OBJECT_API_NAMESPACES=" in f" {text}"
        named_config = NONE
        for word in text.split():
            if word.startswith("STADO_CONFIG="):
                named_config = word.split("=", len("a"))[-1]
        findings.append((pid, program.strip()[: len("a" * 100)], carries_env, named_config))
    return findings


def prefixes_of(entry):
    """Every prefix this namespace grants, in whichever shape it is written."""
    if "prefix_policies" in entry:
        return [str(policy.get("prefix", "")) for policy in entry["prefix_policies"]]
    return [str(prefix) for prefix in entry.get("prefixes", [])]


def granted(entry):
    """The entry with both declaration prefixes added, or None when it has them."""
    missing = [prefix for prefix in GRANTED if prefix not in prefixes_of(entry)]
    if not missing:
        return NONE
    updated = json.loads(json.dumps(entry))
    if "prefix_policies" in updated:
        model = next(
            (policy for policy in updated["prefix_policies"] if policy.get("prefix") == MODEL),
            NONE,
        )
        if model is NONE:
            raise SystemExit(
                f"{KEY}.prefix_policies has no {MODEL} policy to copy; "
                "refusing to invent a verb set for declaration objects"
            )
        copies = []
        for prefix in missing:
            copied = json.loads(json.dumps(model))
            copied["prefix"] = prefix
            copies.append(copied)
        updated["prefix_policies"] = sorted(
            updated["prefix_policies"] + copies, key=lambda policy: policy.get("prefix", "")
        )
        return updated
    # The legacy shape grants one action set to every prefix, so the declaration
    # prefixes and `host_health/` are governed identically by construction.
    if MODEL not in updated.get("prefixes", []):
        raise SystemExit(
            f"{KEY}.prefixes does not list {MODEL}; this is not the namespace that "
            "carries the fleet's per-host documents"
        )
    updated["prefixes"] = sorted(updated["prefixes"] + missing)
    return updated


def main():
    path = config_path()
    print(f"config      {path}")
    if not path.is_file():
        raise SystemExit(f"{path} does not exist; this host serves no object API")
    port = declared_port()
    print(f"endpoint    port {port or '(this host declares no object-API endpoint)'}")
    for pid, program, carries_env, named in serving_process_environment(port):
        print(f"listener    pid {pid}  {program}")
        print(f"  config    {named or '(uses the account default)'}")
        print(f"  env map   WC_OBJECT_API_NAMESPACES {'set' if carries_env else 'unset'}")
        if carries_env:
            raise SystemExit(
                "the live object API carries WC_OBJECT_API_NAMESPACES, which overrides the "
                "config file; grant the prefix there or the edit will validate and do nothing"
            )

    document = json.loads(path.read_text(encoding="utf-8"))
    entry = document.get("object_api", {}).get("namespaces", {}).get(NAMESPACE)
    if entry is NONE:
        raise SystemExit(f"{path} declares no {KEY}; this host does not gate that namespace")
    print(f"item        {entry.get('item')}")
    print(f"before      {' '.join(prefixes_of(entry))}")

    updated = granted(entry)
    if updated is NONE:
        print(f"after       {' '.join(prefixes_of(entry))}")
        print(f"settled     {' '.join(GRANTED)} are already granted in {NAMESPACE}")
        return NONE

    environment = dict(os.environ, STADO_CONFIG=str(path))
    code, output = run(
        str(STADO), "config", "set", KEY, json.dumps(updated), env=environment
    )
    if code != ZERO:
        raise SystemExit(f"stado config set {KEY} failed: {output}")
    after = json.loads(path.read_text(encoding="utf-8"))["object_api"]["namespaces"][NAMESPACE]
    print(f"after       {' '.join(prefixes_of(after))}")
    code, output = run(str(STADO), "config", "validate", env=environment)
    print(f"validate    exit {code}  {output.splitlines()[-len('a'):] or ['(silent)']}")
    print("restart     the object API resolves this map once per process; it must be restarted")
    return NONE


sys.exit(main())
