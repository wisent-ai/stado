#!/usr/bin/env python3
"""Exit successfully when ACTUAL is a valid SemVer at least MINIMUM."""

from __future__ import annotations

import re
import sys

SEMVER = re.compile(
    r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$"
)


def parse(value: str) -> tuple[tuple[int, int, int], list[str] | None]:
    match = SEMVER.fullmatch(value)
    if not match:
        raise ValueError(f"invalid semantic version: {value}")
    core = tuple(int(match.group(index)) for index in range(1, 4))
    prerelease = match.group(4)
    return core, prerelease.split(".") if prerelease else None


def prerelease_compare(left: list[str] | None, right: list[str] | None) -> int:
    if left is None or right is None:
        return (left is None) - (right is None)
    for a, b in zip(left, right):
        if a == b:
            continue
        if a.isdigit() and b.isdigit():
            return (int(a) > int(b)) - (int(a) < int(b))
        if a.isdigit() != b.isdigit():
            return -1 if a.isdigit() else 1
        return (a > b) - (a < b)
    return (len(left) > len(right)) - (len(left) < len(right))


def at_least(actual: str, minimum: str) -> bool:
    actual_core, actual_pre = parse(actual)
    minimum_core, minimum_pre = parse(minimum)
    if actual_core != minimum_core:
        return actual_core > minimum_core
    return prerelease_compare(actual_pre, minimum_pre) >= 0


def main() -> None:
    if len(sys.argv) != 3:
        raise SystemExit("usage: semver_at_least.py ACTUAL MINIMUM")
    try:
        accepted = at_least(sys.argv[1], sys.argv[2])
    except ValueError as error:
        print(error, file=sys.stderr)
        raise SystemExit(2) from error
    raise SystemExit(0 if accepted else 1)


if __name__ == "__main__":
    main()
