#!/usr/bin/env python3
"""Name a fleet host's operating-system account in the registry, and nowhere else.

A machine account is a credential, so it belongs in Skarbiec as a `host-account`
item. What the registry owes the fleet is the pointer: `account_ref` on the
target names the item id, so a reader that has a host name can find the account
without guessing an id or being told one in a transcript.

The registry is the fleet's most-read document, so this edits the canonical text
rather than reserializing it: the pulled bytes are changed by exactly one
inserted line and are otherwise identical, which keeps the diff reviewable and
keeps key order stable for every other reader. Running it twice changes nothing.
"""

import json
import os
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
ONE = len("a")
HOME = pathlib.Path(os.path.expanduser("~"))
STADO = HOME / ".stado" / "bin" / "stado"
WORK = HOME / ".stado"
FIELD = "account_ref"
TARGET_INDENT = " " * len("    ")
MEMBER_INDENT = " " * len("      ")
ENVIRONMENT = {**os.environ, "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"}


def run(*args, stdin=NONE):
    return subprocess.run(
        args, capture_output=True, text=True, input=stdin, check=False, env=ENVIRONMENT
    )


def target_block(lines, name):
    """The half-open line range of the registry target called `name`."""
    marker = f'{MEMBER_INDENT}"name": "{name}",'
    for index, line in enumerate(lines):
        if line != marker:
            continue
        start = index
        while start > ZERO and lines[start].strip() != "{":
            start -= ONE
        stop = index
        while stop < len(lines) and not lines[stop].startswith(f"{TARGET_INDENT}}}"):
            stop += ONE
        return start, stop
    raise SystemExit(f"no registry target named {name}")


def main():
    if len(sys.argv) != len(["self", "target", "item"]):
        raise SystemExit("usage: declare-host-account-ref.py <registry-target> <item-id>")
    name, item = sys.argv[ONE], sys.argv[ONE + ONE]

    pulled = run(str(STADO), "registry", "pull")
    if pulled.returncode != ZERO:
        raise SystemExit(f"registry pull failed: {pulled.stderr.strip().splitlines()[-1:]}")
    canonical = pulled.stdout
    lines = canonical.splitlines()
    start, stop = target_block(lines, name)

    document = json.loads(canonical)
    declared = next(
        entry.get(FIELD)
        for entry in document.get("targets", [])
        if entry.get("name") == name
    )
    print(f"target       {name}")
    print(f"registry     {len(canonical)} bytes, target on lines {start + ONE}-{stop}")
    print(f"declared     {declared or '(absent)'}")
    print(f"requested    {item}")
    if declared == item:
        print("settled      the target already names this item; nothing written")
        return NONE
    if declared:
        raise SystemExit(
            f"refusing to retarget {name} from {declared} to {item}; "
            "remove the old declaration deliberately first"
        )

    # `ssh` is the account this fleet already reaches the host with, so the item
    # that holds that account's password reads best on the next line.
    anchor = next(
        index
        for index in range(start, stop)
        if lines[index].startswith(f'{MEMBER_INDENT}"ssh":')
    )
    edited = lines[: anchor + ONE]
    edited.append(f'{MEMBER_INDENT}"{FIELD}": "{item}",')
    edited += lines[anchor + ONE :]
    candidate = "\n".join(edited) + "\n"

    before = WORK / f"registry.json.before-{name}-{FIELD}"
    proposed = WORK / f"registry.json.with-{name}-{FIELD}"
    before.write_text(canonical, encoding="utf-8")
    proposed.write_text(candidate, encoding="utf-8")
    print(f"anchor       inserted after line {anchor + ONE}: {lines[anchor].strip()}")
    print(f"candidate    {len(candidate)} bytes at {proposed}")
    print(f"delta        {len(candidate) - len(canonical)} bytes, {len(edited) - len(lines)} line")

    checked = run(str(STADO), "registry", "validate", str(proposed))
    print(f"validate     rc={checked.returncode} {checked.stdout.strip()[: len('a' * 200)]}")
    if checked.returncode != ZERO:
        raise SystemExit(f"validate refused: {checked.stderr.strip().splitlines()[-1:]}")

    pushed = run(str(STADO), "registry", "push", str(proposed))
    print(f"push         rc={pushed.returncode} {pushed.stdout.strip()[: len('a' * 200)]}")
    if pushed.returncode != ZERO:
        raise SystemExit(f"push refused: {pushed.stderr.strip().splitlines()[-1:]}")

    settled = run(str(STADO), "registry", "pull")
    after = json.loads(settled.stdout)
    live = next(
        entry.get(FIELD) for entry in after.get("targets", []) if entry.get("name") == name
    )
    print(f"canonical    {name}.{FIELD} = {live or '(absent)'}")
    return NONE


sys.exit(main())
