#!/usr/bin/env python3
"""Allow enrollment objects under the fleet's own queue namespace.

`stado fleet invite`, the machine-filed join requests, and the published
ingress address all live under `enrollments/` inside the storage namespace the
fleet already uses (`probierz`). The object API authorizes writes per declared
prefix, `enrollments/` was never declared, and so every enrollment write —
including the pre-existing `stado fleet join` path, which nobody had ever
exercised — answered `401 unauthorized or non-immutable release write`.

Adds `enrollments/` to `.object_api.namespaces.probierz.prefixes` in this
host's control-plane config. Idempotent: a second run changes nothing and says
so. The config is copied beside itself first, and the write is atomic
(tempfile + rename), because this file is read by the object API at startup
and a half-written config is a fleet-wide outage.

Runs as a Stado helper (`install-helper` + `run-helper`), which passes no
arguments; the process serving the config must be restarted afterwards — the
namespace table is resolved once per process.
"""

import json
import os
import pathlib
import shutil
import tempfile
import time

NONE = None
HOME = pathlib.Path(os.path.expanduser("~"))
CONFIG = HOME / ".config" / "stado" / "config.json"
NAMESPACE = "probierz"
PREFIX = "enrollments/"


def main():
    if not CONFIG.is_file():
        raise SystemExit(f"no control-plane config at {CONFIG}")
    document = json.loads(CONFIG.read_text())
    namespaces = document.get("object_api", {}).get("namespaces", {})
    policy = namespaces.get(NAMESPACE)
    if policy is NONE:
        raise SystemExit(
            f"namespace {NAMESPACE!r} is not declared in {CONFIG}; refusing to invent it"
        )
    prefixes = policy.setdefault("prefixes", [])
    print(f"config     {CONFIG}")
    print(f"namespace  {NAMESPACE} (item {policy.get('item')!r})")
    print(f"prefixes   {len(prefixes)} declared")
    if PREFIX in prefixes:
        print(f"unchanged  {PREFIX!r} is already declared; nothing written")
        return NONE
    stamp = time.strftime("%Y%m%dT%H%M%S")
    backup = CONFIG.with_name(f"config.json.before-enrollments-{stamp}")
    shutil.copy2(CONFIG, backup)
    prefixes.append(PREFIX)
    prefixes.sort()
    fd, staged = tempfile.mkstemp(dir=str(CONFIG.parent), prefix=".config-enrollments-")
    with os.fdopen(fd, "w") as handle:
        json.dump(document, handle, indent=2, sort_keys=True)
        handle.write("\n")
    os.chmod(staged, CONFIG.stat().st_mode & 0o777)
    os.replace(staged, CONFIG)
    # Read back through the same parser: an unparsable config would take the
    # object API down at its next start, which is exactly when it reads it.
    reread = json.loads(CONFIG.read_text())
    declared = reread["object_api"]["namespaces"][NAMESPACE]["prefixes"]
    if PREFIX not in declared:
        shutil.copy2(backup, CONFIG)
        raise SystemExit("read-back failed; the previous config was restored")
    print(f"backup     {backup}")
    print(f"written    {PREFIX!r} added; {len(declared)} prefixes now declared")
    print("note       the object API resolves namespaces once per process; restart it")
    return NONE


main()
