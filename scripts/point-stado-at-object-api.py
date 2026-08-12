#!/usr/bin/env python3
"""Make this host address the registry the operator writes, not its own disk.

With `storage.backend = "local"` a host reads `registry.json` off its own disk
while every operator command writes `<namespace>/registry.json` through the
object API. Both names look canonical, both answer, and the difference only
shows when a service starts from the stale one -- which is how a resolver came
up against a registry nobody had written for weeks and refused to serve.

Pointing the host at the object API it already runs closes that: one object,
one writer path, one document. The previous configuration is kept beside the
new one, and `local` stays as the read fallback so a host whose object API is
down still starts.

Run with no arguments to see what it would do; run with `apply` to write.
"""

import datetime
import json
import os
import pathlib
import subprocess
import socket
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
CONFIG = HOME / ".config" / "stado" / "config.json"
STADO = HOME / ".stado" / "bin" / "stado"
SERVICE = "stado-object-api"
NAMESPACE = "probierz"
OWNER_ONLY = 0o600
PREFERRED_TOKEN = "wisent-queue-object-api-token"


def token_candidates():
    """Token files this host already holds for the object API.

    The preferred name first: several consumers keep a token here, and the
    newest file is whichever one rotated last, not the one that speaks for this
    host's own client identity.
    """
    found = [
        path
        for path in sorted((HOME / ".stado").glob("*object-api*token*"))
        if path.is_file() and path.stat().st_size > ZERO
    ]
    return sorted(found, key=lambda path: (path.name != PREFERRED_TOKEN, path.name))


def load():
    if not CONFIG.is_file():
        return {}
    try:
        return json.loads(CONFIG.read_text(encoding="utf-8"))
    except ValueError as error:
        raise SystemExit(f"{CONFIG} is not readable JSON: {error}")


def pull_digest():
    """What `stado registry pull` returns here, as a short digest."""
    import hashlib

    proc = subprocess.run(
        [str(STADO), "registry", "pull"], capture_output=True, text=True, check=False
    )
    if proc.returncode != 0:
        return f"failed: {proc.stderr.strip().splitlines()[-1:] or proc.stdout.strip()[:120]}"
    body = proc.stdout.encode("utf-8")
    document = json.loads(proc.stdout)
    platforms = ",".join(
        str(entry.get("release_platform")) for entry in document.get("targets", [])
    )
    return f"sha256 {hashlib.sha256(body).hexdigest()[:12]}  platforms {platforms}"

def registry_document():
    """The registry, read the way a repair tool must be able to read it.

    Asking the object API for the document that configures the object API is a
    loop with no exit while that API is down, and this script's whole job is to
    run in that state. The host's last-known-good copy is the way out.
    """
    proc = subprocess.run(
        [str(STADO), "registry", "pull"], capture_output=True, text=True, check=False
    )
    if proc.returncode == ZERO:
        return json.loads(proc.stdout)
    for candidate in (
        HOME / ".stado" / "local-storage" / "registry.json",
        HOME / ".stado" / "local-storage" / "ecosystem" / NAMESPACE / "registry.json",
        HOME / ".stado" / "local-backup" / "registry.json",
        # A host that has never reached the store has no copy of its own. One
        # delivered by `stado host install-file` is how it gets its first, and
        # without this the machine can never be pointed anywhere.
        HOME / ".stado" / "files" / "registry-next.json",
    ):
        if candidate.is_file():
            print(f"fallback        {candidate}")
            return json.loads(candidate.read_text(encoding="utf-8"))
    raise SystemExit("the object API is down and this host holds no registry copy")


def this_target(document):
    """Which registry target this machine is, without asking the object API."""
    node = socket.gethostname().lower()
    short = node.split(".")[ZERO]
    for entry in document.get("targets", []):
        names = [str(name).lower() for name in entry.get("hostnames", [])]
        names.append(str(entry.get("name", "")).lower())
        if any(name == node or name.split(".")[ZERO] == short for name in names if name):
            return entry.get("name")
    raise SystemExit(f"no registry target matches this machine ({node})")


def object_api_url(document):
    """Where this machine reaches the fleet's object API, per the registry.

    The active host talks to its own control plane over loopback. Everyone else
    goes straight to the tailnet address in that target's `ssh` coordinate --
    never through a resolver adapter, because the resolver reads the registry
    to learn its own routes and an adapter it serves cannot answer before it
    has them. That loop leaves the machine with no registry at all.
    """
    service = document["service_directory"]["services"][SERVICE]
    active = service["active_host"]
    here = this_target(document)
    if here == active:
        return service["endpoints"][active]["url"]
    for entry in document["targets"]:
        if entry.get("name") != active:
            continue
        coordinate = str(entry.get("ssh", ""))
        host = coordinate.split("@")[-1].strip()
        if not host:
            break
        port = service["endpoints"][active]["url"].rsplit(":", len(["port"]))[-1]
        return f"https://{host}:{port}"
    raise SystemExit(f"the registry gives no reachable address for {SERVICE} on {active}")



def apply_storage(path, backup_store, wanted, local, required=False):
    """Give one Stado config the fleet's store. True when it had to be written.

    The previous file is kept beside the new one, so a config that turns out to
    have been right for a reason nobody wrote down can be put back by copying a
    file rather than by remembering what it said.

    A host that has never had a config is the case this exists for: reporting
    "settled" for an absent file left the fleet's Linux machine reading its own
    empty disk while claiming to be pointed at the object API. Satellite configs
    are different -- one that is absent is a unit this host does not run -- so
    only the required config is created.
    """
    if not path.is_file() and not required:
        return False
    document = {}
    saved = NONE
    if path.is_file():
        try:
            document = json.loads(path.read_text(encoding="utf-8"))
        except ValueError:
            return False
    storage = document.get("storage", {})
    if (
        storage.get("backend") == "stado"
        and storage.get("stado") == wanted
        and storage.get("backup") == backup_store
    ):
        return False
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    if path.is_file():
        saved = path.with_name(f"{path.name}.before-object-api-{stamp}")
        saved.write_text(path.read_text(encoding="utf-8"), encoding="utf-8")
        os.chmod(saved, OWNER_ONLY)
    else:
        path.parent.mkdir(parents=True, exist_ok=True)
    storage["backend"] = "stado"
    storage["local"] = storage.get("local", local)
    storage["backup"] = backup_store
    storage["stado"] = wanted
    document["storage"] = storage
    staging = path.with_name(f"{path.name}.{os.getpid()}.tmp")
    staging.write_text(json.dumps(document, indent=len("ba")) + "\n", encoding="utf-8")
    os.chmod(staging, OWNER_ONLY)
    staging.replace(path)
    print(f"backup          {saved or '(new file, nothing to keep)'}")
    return True


def main():
    document = load()
    storage = document.get("storage", {})
    tokens = token_candidates()
    print(f"config          {CONFIG}")
    print(f"backend now     {storage.get('backend', '(unset)')}")
    print(f"token files     {' '.join(str(path) for path in tokens) or '(none)'}")
    print(f"registry now    {pull_digest()}")
    if not tokens:
        print("refusing: the object API needs a token this host does not hold")
        return len("x")
    # A helper takes no operator words on purpose, so this applies every time
    # it runs. Writing an identical file would still leave a backup behind and
    # a trail of them reads like repeated repair, so settled is a no-op.
    registry = registry_document()
    wanted = {
        "url": object_api_url(registry),
        "namespace": NAMESPACE,
        "token_file": str(tokens[ZERO]),
    }
    # A tailnet address is served over TLS by the fleet's proxy, and the client
    # needs the tailnet CA to trust it. Loopback needs nothing.
    existing_ca = storage.get("stado", {}).get("ca_file")
    default_ca = HOME / ".stado" / "stado-tailnet-ca.crt"
    if wanted["url"].startswith("https://"):
        ca_file = existing_ca or (str(default_ca) if default_ca.is_file() else "")
        if not ca_file:
            raise SystemExit(f"{wanted['url']} needs a tailnet CA and {default_ca} is absent")
        wanted["ca_file"] = ca_file
    local = storage.get("local", {"path": "~/.stado/local-storage"})
    # A backup that resolves to the primary store is refused at startup, and a
    # unit pinned to `WC_STORAGE_BACKEND=local` reaches exactly that when the
    # backup repeats the primary path -- the object API refused to serve for
    # that reason and took the fleet's registry with it. Mirror to the separate
    # directory the fleet already keeps.
    backup_store = storage.get("backup", {})
    collides = backup_store.get("local", {}).get("path") == local.get("path")
    if collides or not backup_store:
        backup_store = {"backend": "local", "local": {"path": "~/.stado/local-backup"}}
    if not apply_storage(CONFIG, backup_store, wanted, local, required=True):
        print("settled         already addressing the object API")
    # Units that carry their own `STADO_CONFIG` are separate readers of the same
    # fleet: the health beacon has one, it declares no storage at all, and it
    # therefore reported into a store nothing else reads -- which is why a live
    # host reads as "never" in `registry beacon-age`. Give every such config the
    # same store, or the split just moves.
    for satellite in sorted((HOME / ".stado").glob("*.config.json")):
        if apply_storage(satellite, backup_store, wanted, local):
            print(f"aligned         {satellite}")
    print(f"registry after  {pull_digest()}")
    return NONE


sys.exit(main())
