#!/usr/bin/env python3
"""Print this host's storage binding: which backend its CLIs resolve and where.

Read-only. Exists because the operator bridge on the always-on host spawned
CLI children that inherited the dashboard's WC_STORAGE_BACKEND=local override
and silently read bare paths at the store root, while the same process served
every remote writer namespaced blobs — two views of one disk, disagreeing.
Secret values are never printed; only paths and backend names.
"""

import json
import os
import pathlib

HOME = pathlib.Path(os.path.expanduser("~"))
CONFIG = HOME / ".config" / "stado" / "config.json"


def main():
    if not CONFIG.is_file():
        print(f"(no config at {CONFIG})")
        return
    document = json.loads(CONFIG.read_text())
    storage = document.get("storage", {})
    shown = {}
    for backend, section in storage.items():
        if backend.startswith("_"):
            continue
        if isinstance(section, dict):
            shown[backend] = {
                key: value
                for key, value in section.items()
                if not key.startswith("_") and "token" not in key.lower()
            }
        else:
            shown[backend] = section
    print(json.dumps(shown, indent=2, sort_keys=True))


main()
