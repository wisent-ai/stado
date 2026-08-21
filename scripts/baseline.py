#!/usr/bin/env python3
"""Select the best reachable Stado release baseline identity."""

from __future__ import annotations

import argparse
import json
import re
import subprocess
from pathlib import Path

TAG = re.compile(r"^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z.-]+))?$")


def command(*args: str) -> str:
    return subprocess.run(
        args,
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    ).stdout


def version_key(tag: str) -> tuple[int, int, int, int, str]:
    match = TAG.fullmatch(tag)
    if not match:
        raise ValueError(tag)
    prerelease = match.group(4)
    return (
        int(match.group(1)),
        int(match.group(2)),
        int(match.group(3)),
        1 if prerelease is None else 0,
        prerelease or "",
    )


def visible_tags() -> list[str]:
    if command("git", "rev-parse", "--is-shallow-repository").strip() == "true":
        raise SystemExit("repository is shallow; release tags are not fully visible")
    local = {tag for tag in command("git", "tag", "--list", "v*").splitlines() if TAG.fullmatch(tag)}
    remote = {
        ref.removeprefix("refs/tags/").removesuffix("^{}")
        for line in command("git", "ls-remote", "--tags", "origin").splitlines()
        for ref in [line.split("\t", 1)[1]]
        if TAG.fullmatch(ref.removeprefix("refs/tags/").removesuffix("^{}"))
    }
    missing = remote - local
    if missing:
        raise SystemExit(f"release tags are not fully visible: missing {sorted(missing)}")
    return sorted(local, key=version_key, reverse=True)


def declared_versions() -> set[str]:
    versions: set[str] = set()
    cargo = Path("stado-rs/Cargo.toml")
    if cargo.is_file():
        match = re.search(
            r'^version\s*=\s*"([^"]+)"\s*$',
            cargo.read_text(encoding="utf-8"),
            re.MULTILINE,
        )
        if match and TAG.fullmatch(f"v{match.group(1)}"):
            versions.add(match.group(1))
    baseline = Path("released-surface.json")
    if baseline.is_file():
        value = json.loads(baseline.read_text(encoding="utf-8")).get("version")
        if isinstance(value, str) and TAG.fullmatch(f"v{value}"):
            versions.add(value)
    return versions


def state(stado: Path, uri: str) -> str:
    result = subprocess.run(
        [str(stado), "storage", "stat", uri, "--json"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    if result.returncode:
        raise SystemExit(result.stderr.strip() or f"storage stat failed for {uri}")
    try:
        return str(json.loads(result.stdout).get("state", ""))
    except json.JSONDecodeError as error:
        raise SystemExit(f"storage stat returned invalid JSON for {uri}") from error


def best(stado: Path) -> str:
    tags = visible_tags()
    versions = {tag[1:] for tag in tags} | declared_versions()
    for version in sorted(versions, key=lambda value: version_key(f"v{value}"), reverse=True):
        uri = f"stado://releases/stado/{version}/linux-amd64/release-manifest-linux-amd64.json"
        verdict = state(stado, uri)
        if verdict == "present":
            return f"stado:{version}"
        if verdict != "absent":
            raise SystemExit(f"release channel returned state {verdict!r} for {uri}")
    if tags:
        return f"git-archive:{tags[0]}"
    return "head"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--best", action="store_true", required=True)
    parser.add_argument("--stado", type=Path, required=True)
    args = parser.parse_args()
    print(best(args.stado))


if __name__ == "__main__":
    main()
