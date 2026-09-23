"""Print the public surface of the `stado` binary: the commands it advertises.

What a caller of this product depends on is not the crate's Rust symbols — nothing
links against it — but **which commands the binary offers**. Adding one is a
capability; removing one breaks every script that invoked it yesterday. So the
advertised command list is the public contract, and this prints it for the shared
versioning rule to compare. The rule itself is not here:
https://github.com/lbartoszcze/AutoVersion.

Read from the artifact's own `help`, never from `src/cli/mod.rs`. Help output *is* the
promise: a variant marked `hide = true` dispatches but is not offered, so it is
private and must not be counted, and reading the source would count it. Running the
artifact also means the surface of an already published release can be recovered
exactly instead of assumed — point `--binary` at a downloaded release binary and this
answers for that version.

`help` is excluded. clap injects it into every application, so it says nothing about
this product.

Usage:
    python3 scripts/surface.py [--binary PATH | --help-text FILE]
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import subprocess
import sys

FIRST = int(False)
ONE = int(True)
INDENT = ONE + ONE
OK = FIRST

DEFAULT_BINARY = "stado-rs/target/release/stado"
COMMANDS_HEADING = "Commands:"

# clap indents each command by exactly two spaces and aligns wrapped description
# text under the description column, far to the right. So a line that starts with
# exactly two spaces and then a non-space is a command entry and nothing else. The
# name is taken verbatim rather than matched against a character class, because a
# class that misses a future name would silently shrink the surface instead of
# failing.
ENTRY = re.compile(r"^ {2}(\S+)(?:\s{2,}\S.*)?$")
INJECTED = frozenset({"help"})


def advertised(text: str) -> list:
    """The command names one clap help screen offers, sorted and unique."""
    names = set()
    inside = False
    for line in text.splitlines():
        if not inside:
            inside = line.strip() == COMMANDS_HEADING
            continue
        if not line.strip():
            break
        found = ENTRY.match(line)
        if found is not None:
            names.add(found.group(ONE))
    return sorted(names - INJECTED)


def help_text(binary: pathlib.Path) -> str:
    """What one binary prints when asked to describe itself.

    `help` and `--help` print the same screen; `help` is used because it is the
    shape a user is told about in the onboarding text.
    """
    finished = subprocess.run(
        [str(binary), "help"],
        capture_output=True,
        text=True,
        check=False,
    )
    if finished.returncode != OK:
        raise SystemExit(
            f"{binary} help exited {finished.returncode}: {finished.stderr.strip()}"
        )
    return finished.stdout


def surface(binary: pathlib.Path | None, text: pathlib.Path | None) -> list:
    """The advertised commands, from a binary or from captured help output."""
    captured = text.read_text(encoding="utf-8") if text is not None else help_text(binary)
    return advertised(captured)


def main(argv: list) -> int:
    parser = argparse.ArgumentParser(
        description="Print the commands the stado binary advertises."
    )
    parser.add_argument(
        "--binary",
        default=DEFAULT_BINARY,
        help=f"stado binary to ask (default {DEFAULT_BINARY})",
    )
    parser.add_argument(
        "--help-text",
        dest="help_file",
        default=None,
        help="read captured `stado help` output from this file instead of running a binary",
    )
    args = parser.parse_args(argv[ONE:])

    captured = pathlib.Path(args.help_file) if args.help_file else None
    names = surface(pathlib.Path(args.binary), captured)
    if not names:
        source = args.help_file or args.binary
        print(
            f"{source} advertises no commands; refusing to report an empty surface",
            file=sys.stderr,
        )
        return ONE
    print(json.dumps({"surface": names}, indent=INDENT))
    return OK


if __name__ == "__main__":
    sys.exit(main(sys.argv))
