#!/usr/bin/env python3
"""Emit the public top-level command surface advertised by a CLI binary."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
from pathlib import Path

COMMAND = re.compile(r"^  ([a-z0-9][a-z0-9-]*)\s{2,}\S")


def advertised_commands(help_text: str) -> list[str]:
    commands: list[str] = []
    in_commands = False
    for line in help_text.splitlines():
        if line == "Commands:":
            in_commands = True
            continue
        if not in_commands:
            continue
        if line and not line.startswith(" "):
            break
        match = COMMAND.match(line)
        if match and match.group(1) != "help":
            commands.append(match.group(1))
    if not commands:
        raise SystemExit("CLI help contains no advertised Commands section")
    return sorted(set(commands))


def main() -> None:
    parser = argparse.ArgumentParser()
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--binary", type=Path)
    source.add_argument("--help-text", type=Path)
    args = parser.parse_args()

    if args.binary:
        result = subprocess.run(
            [str(args.binary), "help"],
            check=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        help_text = result.stdout
    else:
        help_text = args.help_text.read_text(encoding="utf-8")

    print(json.dumps({"surface": advertised_commands(help_text)}, indent=2))


if __name__ == "__main__":
    main()
