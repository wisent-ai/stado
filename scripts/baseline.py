#!/usr/bin/env python3
"""Derive the Stado command-surface baseline from verified release bytes."""

from __future__ import annotations

import argparse
import hashlib
import json
import platform
import re
import subprocess
import tarfile
import tempfile
from pathlib import Path

TAG = re.compile(r"^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z.-]+))?$")
HEX = re.compile(r"^[0-9a-fA-F]+$")


def command(*args: str, cwd: Path | None = None) -> str:
    return subprocess.run(
        args,
        cwd=cwd,
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


def declared_version() -> str | None:
    cargo = Path("stado-rs/Cargo.toml")
    if not cargo.is_file():
        return None
    match = re.search(
        r'^version\s*=\s*"([^"]+)"\s*$',
        cargo.read_text(encoding="utf-8"),
        re.MULTILINE,
    )
    if match and TAG.fullmatch(f"v{match.group(1)}"):
        return match.group(1)
    return None


def native_platform() -> str:
    system = platform.system().lower()
    machine = platform.machine().lower()
    if system == "darwin" and machine in {"arm64", "aarch64"}:
        return "darwin-arm64"
    if system == "linux" and machine in {"x86_64", "amd64"}:
        return "linux-amd64"
    raise SystemExit(f"no executable Stado release platform for {system}/{machine}")


def run_stado(stado: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(stado), *args],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )


def state(stado: Path, uri: str) -> str:
    result = run_stado(stado, "storage", "stat", uri, "--json")
    if result.returncode:
        raise SystemExit(result.stderr.strip() or f"storage stat failed for {uri}")
    try:
        return str(json.loads(result.stdout).get("state", ""))
    except json.JSONDecodeError as error:
        raise SystemExit(f"storage stat returned invalid JSON for {uri}") from error


def get(stado: Path, uri: str, destination: Path) -> None:
    result = run_stado(stado, "storage", "get", uri, str(destination))
    if result.returncode:
        raise SystemExit(result.stderr.strip() or f"storage get failed for {uri}")


def release_base(version: str, release_platform: str) -> str:
    return f"stado://releases/stado/{version}/{release_platform}"


def manifest_for(stado: Path, version: str, release_platform: str, root: Path) -> tuple[dict[str, str], str]:
    uri = f"{release_base(version, release_platform)}/release-manifest-{release_platform}.json"
    destination = root / f"manifest-{release_platform}.json"
    get(stado, uri, destination)
    try:
        value = json.loads(destination.read_text(encoding="utf-8"))
    except json.JSONDecodeError as error:
        raise SystemExit(f"release channel returned invalid JSON for {uri}") from error
    fields = {"platform", "product", "sha256", "source_commit", "version"}
    if not isinstance(value, dict) or set(value) != fields:
        raise SystemExit(f"release manifest has unexpected fields: {uri}")
    if (value["product"], value["version"], value["platform"]) != ("stado", version, release_platform):
        raise SystemExit(f"release manifest identity mismatch: {uri}")
    digest = value["sha256"]
    commit = value["source_commit"]
    if not isinstance(digest, str) or len(digest) != 64 or not HEX.fullmatch(digest):
        raise SystemExit(f"release manifest digest is invalid: {uri}")
    if not isinstance(commit, str) or len(commit) not in {40, 64} or not HEX.fullmatch(commit):
        raise SystemExit(f"release manifest source commit is invalid: {uri}")
    return value, uri


def safe_extract(archive: Path, destination: Path) -> None:
    destination = destination.resolve()
    with tarfile.open(archive, "r:gz") as bundle:
        for member in bundle.getmembers():
            target = (destination / member.name).resolve()
            if destination != target and destination not in target.parents:
                raise SystemExit(f"release archive contains an unsafe path: {member.name}")
        bundle.extractall(destination, filter="data")


def surface_from_release(stado: Path, version: str, release_platform: str, root: Path) -> dict[str, object]:
    manifest, manifest_uri = manifest_for(stado, version, release_platform, root)
    archive_uri = f"{release_base(version, release_platform)}/stado-v{version}-{release_platform}.tar.gz"
    archive = root / "release.tar.gz"
    get(stado, archive_uri, archive)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    if digest != manifest["sha256"]:
        raise SystemExit(f"release archive differs from its manifest: {archive_uri}")
    extracted = root / "release"
    extracted.mkdir()
    safe_extract(archive, extracted)
    binary = extracted / "stado"
    if not binary.is_file():
        raise SystemExit(f"release archive contains no stado binary: {archive_uri}")
    binary.chmod(binary.stat().st_mode | 0o100)
    surface = json.loads(command("python3", "scripts/surface.py", "--binary", str(binary)))
    commands = surface.get("surface")
    if not isinstance(commands, list) or not commands or not all(isinstance(item, str) for item in commands):
        raise SystemExit(f"release binary advertised an invalid command surface: {archive_uri}")
    return {
        "version": version,
        "source": f"{manifest_uri} published from {manifest['source_commit']}",
        "surface": commands,
    }


def best(stado: Path, output: Path | None) -> str:
    tags = visible_tags()
    versions = {tag[1:] for tag in tags}
    current = declared_version()
    if current is not None:
        versions.add(current)
    release_platform = native_platform()
    # The baseline is a command surface, which is architecture-independent.
    # Verify and execute the release matching this runner. Requiring every
    # platform here made the Linux publisher depend on the later Darwin
    # control-plane job, while that job depends on this publisher succeeding.
    required_platforms = (release_platform,)
    for version in sorted(versions, key=lambda value: version_key(f"v{value}"), reverse=True):
        states = {
            candidate: state(
                stado,
                f"{release_base(version, candidate)}/release-manifest-{candidate}.json",
            )
            for candidate in required_platforms
        }
        if all(verdict == "absent" for verdict in states.values()):
            continue
        unexpected = {name: verdict for name, verdict in states.items() if verdict not in {"present", "absent"}}
        if unexpected:
            raise SystemExit(f"release channel returned unknown platform states for {version}: {unexpected}")
        if any(verdict != "present" for verdict in states.values()):
            continue
        if output is None:
            return f"stado:{version}"
        with tempfile.TemporaryDirectory(prefix="stado-baseline-") as temporary:
            root = Path(temporary)
            manifests = {
                candidate: manifest_for(stado, version, candidate, root)[0]
                for candidate in required_platforms
            }
            commits = {manifest["source_commit"] for manifest in manifests.values()}
            if len(commits) != 1:
                raise SystemExit(f"release platforms for {version} name different source commits")
            baseline = surface_from_release(stado, version, release_platform, root)
        output.write_text(json.dumps(baseline, indent=2) + "\n", encoding="utf-8")
        return f"stado:{version}"
    raise SystemExit("release channel contains no complete verified Stado release")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--best", action="store_true", required=True)
    parser.add_argument("--stado", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    print(best(args.stado, args.output))


if __name__ == "__main__":
    main()
