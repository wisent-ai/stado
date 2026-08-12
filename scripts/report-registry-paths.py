#!/usr/bin/env python3
"""Show every registry document this host can reach, and where it came from.

A host reading `registry.json` off its own disk and an operator writing
`<namespace>/registry.json` through the object API are addressing two different
objects with one name. Both answer, neither is wrong on its face, and the
difference only surfaces when a service starts from the stale one -- which is
the outage this prints in advance.

Read-only: it loads documents and reports, and writes nothing.
"""

import hashlib
import json
import os
import pathlib
import sys

NONE = None
HOME = pathlib.Path(os.path.expanduser("~"))
CONFIG = HOME / ".config" / "stado" / "config.json"
BLOB = "registry.json"


def summarize(label, path):
    if not path.is_file():
        print(f"{label:<28} {path}  (absent)")
        return
    raw = path.read_bytes()
    digest = hashlib.sha256(raw).hexdigest()[: len("aaaaaaaaaaaa")]
    try:
        document = json.loads(raw)
    except ValueError as error:
        print(f"{label:<28} {path}  unreadable: {error}")
        return
    targets = document.get("targets", [])
    names = ",".join(str(entry.get("name")) for entry in targets)
    platforms = ",".join(str(entry.get("release_platform")) for entry in targets)
    services = sum(len(entry.get("services", [])) for entry in targets)
    print(f"{label:<28} {path}")
    print(f"{'':<28} sha256 {digest}  targets {names or '(none)'}")
    print(f"{'':<28} platforms {platforms or '(none)'}  services {services}")


def storage():
    if not CONFIG.is_file():
        return {}
    try:
        return json.loads(CONFIG.read_text(encoding="utf-8")).get("storage", {})
    except ValueError:
        return {}


def main():
    settings = storage()
    backend = settings.get("backend", "(unset)")
    namespace = settings.get("stado", {}).get("namespace", "")
    local_root = pathlib.Path(
        os.path.expanduser(settings.get("local", {}).get("path", "~/.stado/local-storage"))
    )
    report(backend, namespace, local_root)
    return NONE


def report(backend, namespace, local_root):
    print(f"configured backend {backend}  namespace {namespace or '(none)'}")
    summarize("local bare", local_root / BLOB)
    if namespace:
        summarize("local under namespace", local_root / namespace / BLOB)
    # Whoever serves the object API keeps its objects somewhere of its own
    # choosing, and a name that is absent where this host looks is not a name
    # that is absent. Show every copy under the Stado directory so the two
    # readers can be compared instead of assumed identical.
    seen = {local_root / BLOB, local_root / namespace / BLOB}
    for found in sorted((HOME / ".stado").rglob(BLOB)):
        if found not in seen:
            summarize("also on disk", found)
    print(
        "verdict one store"
        if backend == "stado"
        else "verdict this host reads its own disk while the operator reads the object API"
    )

sys.exit(main())
