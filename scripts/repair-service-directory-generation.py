#!/usr/bin/env python3
"""Advance the registry's service-directory generation on the store host.

The resolver on every member host refuses a service directory whose content
changed while its generation number stood still — that guard is correct, and
when a writer forgets the bump, every resolver in the fleet parks itself and
each member's data plane goes dark. The store host is the one machine whose
CLI reads the store from disk, so the repair runs here, through the product's
own commands: pull the canonical registry, advance the generation by one,
push it back through full registry validation.

Idempotent in effect: each run advances the generation once, which is exactly
the declaration "the current content is intentional". Run it when resolvers
report "service directory changed without advancing generation N".
"""

import json
import os
import pathlib
import subprocess
import tempfile

STADO = pathlib.Path(os.path.expanduser("~")) / ".stado" / "bin" / "stado"


def run(*argv, **kw):
    return subprocess.run([str(STADO), *argv], capture_output=True, text=True, check=True, **kw)


def main():
    document = json.loads(run("registry", "pull").stdout)
    directory = document.get("service_directory")
    if not isinstance(directory, dict) or "generation" not in directory:
        raise SystemExit("registry carries no service_directory.generation to advance")
    before = directory["generation"]
    directory["generation"] = before + 1
    with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as handle:
        json.dump(document, handle, indent=2)
        staged = handle.name
    try:
        run("registry", "validate", staged)
        run("registry", "push", staged)
    finally:
        os.unlink(staged)
    print(f"service_directory generation: {before} -> {before + 1}")


main()
