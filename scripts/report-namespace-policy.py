#!/usr/bin/env python3
"""Print the object API's namespace policy as this host's config states it.

A read that answers 401 while its neighbour answers 200 is a policy statement,
not a broken service, and the policy lives in one file. Printing the prefixes and
verbs beside the process that read them turns "unauthorized" into a sentence an
operator can act on.

Never prints a secret: item names, prefixes and verbs only.
"""

import json
import os
import pathlib
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
CONFIG = pathlib.Path(os.environ.get("STADO_CONFIG", HOME / ".config" / "stado" / "config.json"))
WATCHED = ("host_capabilities/", "job_requirements/", "host_health/", "capacity/", "registry.json")


def main():
    if not CONFIG.is_file():
        raise SystemExit(f"no config at {CONFIG}")
    document = json.loads(CONFIG.read_text(encoding="utf-8"))
    namespaces = (document.get("object_api") or {}).get("namespaces") or {}
    print(f"config     {CONFIG}")
    print(f"namespaces {sorted(namespaces)}")
    for name, policy in sorted(namespaces.items()):
        entries = policy if isinstance(policy, list) else [policy]
        for entry in entries:
            if not isinstance(entry, dict):
                continue
            prefixes = entry.get("prefixes") or entry.get("prefix_policies") or []
            names = [
                item if isinstance(item, str) else str(item.get("prefix"))
                for item in (prefixes if isinstance(prefixes, list) else [])
            ]
            print(f"  {name}: item={entry.get('item')} actions={entry.get('actions')}")
            print(f"    prefixes {len(names)}: {', '.join(sorted(names))[: len('a' * 400)]}")
            for watched in WATCHED:
                print(f"      {watched:22} {'granted' if watched in names else 'ABSENT'}")
    return NONE


sys.exit(main())
