#!/usr/bin/env python3
"""Point the service directory's authority at a host that can actually serve it.

Every resolver on the fleet asks the authority host for the registry document,
over the registry's own SSH transport. When the authority names a host that
refuses SSH, no resolver anywhere can start, and the failure surfaces at the far
end of the chain as an unreachable Skarbiec or a service that will not boot. An
operator workstation is the wrong authority for exactly that reason: it is the
one machine on the fleet that deliberately does not accept SSH.

This reads the canonical registry, replaces `service_directory.authority` with
the named target and its Stado path, saves the previous document beside the new
one, validates, and pushes only when asked. Reverting is pushing the saved
previous document.

Usage: repoint-registry-authority.py <target> <stado-path> [--push]
"""
import json
import pathlib
import subprocess
import sys

FIRST = len(["argv0"])
NONE = len([])
PAIR = len(["target", "command"])
INDENT = len("ba")
STADO = pathlib.Path.home() / ".stado" / "bin" / "stado"
BEFORE = pathlib.Path("/tmp/registry-authority-before.json")
AFTER = pathlib.Path("/tmp/registry-authority-next.json")


def stado(*arguments):
    return subprocess.run(
        [str(STADO), *arguments], capture_output=True, text=True, check=True
    ).stdout


def main():
    arguments = sys.argv[FIRST:]
    push = "--push" in arguments
    positional = [value for value in arguments if not value.startswith("--")]
    if len(positional) != PAIR:
        raise SystemExit("usage: repoint-registry-authority.py <target> <stado-path> [--push]")
    target, command = positional

    document = json.loads(stado("registry", "pull"))
    directory = document.get("service_directory")
    if not isinstance(directory, dict):
        raise SystemExit("the canonical registry carries no service_directory")

    previous = directory.get("authority")
    BEFORE.write_text(json.dumps(document, indent=INDENT) + "\n", encoding="utf-8")
    directory["authority"] = {"target": target, "command": command}
    AFTER.write_text(json.dumps(document, indent=INDENT) + "\n", encoding="utf-8")

    print(f"before {json.dumps(previous)}")
    print(f"after  {json.dumps(directory['authority'])}")
    print(f"saved  {BEFORE}")
    print(stado("registry", "validate", str(AFTER)).strip())
    print(stado("registry", "push", str(AFTER)).strip() if push else "not pushed; re-run with --push")
    return NONE


sys.exit(main())
