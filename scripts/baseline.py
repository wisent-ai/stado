"""Regenerate released-surface.json: the contract of the version actually published.

Every later release decision is measured against this file, so it must describe a
version that really exists and say truthfully where its surface came from. It is
generated, never hand-written: a hand-written baseline is how a repository ends up
comparing today's surface against a version nobody ever installed.

Tiers, best first. The best tier that exists is used; a lower one is never chosen
because a higher one was inconvenient, and a tier this script cannot reach makes it
refuse loudly instead of quietly degrading:

    stado:<object key>   the artifact published to the Stado release channel
    git-archive:<tag>    a tag rebuilt from `git archive` (no channel artifact yet)
    head:<sha>           last resort: nothing published and no usable tag

The marker is the first whitespace-delimited token of "source"; the rest is prose for
people. `scripts/version_check.sh` reads that token back and asserts it both ways —
a baseline that claims the channel must be served by the channel, and a baseline that
claims nothing must be met by a channel that serves nothing.

A surface is always measured on a *committed* tree (`git archive`), never on the
working copy, so uncommitted work by anyone cannot leak into a published baseline.

Usage:
    python3 scripts/baseline.py [--output released-surface.json] [--stado PATH]
    python3 scripts/baseline.py --print        # show the document, write nothing

Environment:
    CARGO_TARGET_DIR   reused build cache for the archive builds (optional)
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import platform
import shutil
import stat
import subprocess
import sys
import tempfile

from surface import INDENT, OK, ONE, advertised, help_text

REPOSITORY = pathlib.Path(__file__).resolve().parent.parent
MANIFEST = pathlib.Path("stado-rs/Cargo.toml")
BASELINE = pathlib.Path("released-surface.json")
BINARY = "stado"
PRODUCT = "stado"
NAMESPACE = "releases"

MARKER_CHANNEL = "stado:"
MARKER_TAG = "git-archive:"
MARKER_HEAD = "head:"

TRIPLE = ONE + ONE + ONE
KEY_PARTS = TRIPLE + ONE
DEFAULT_TARGET_CACHE = pathlib.Path.home() / ".cache" / "stado-baseline-target"

# The release pipeline publishes exactly these platforms (see
# stado-rs/src/self_update.rs). A published surface can only be read on the platform
# that can execute it.
PLATFORMS = {
    ("Darwin", "arm64"): "darwin-arm64",
    ("Linux", "x86_64"): "linux-amd64",
}

# Any release coordinate serves as the reachability probe; deploy.yml publishes this
# one for every version, so it is the coordinate most likely to exist if anything does.
PROBE_PLATFORM = PLATFORMS[("Linux", "x86_64")]

# Reading a published surface means executing the published bytes, and the host that
# has to read them is not always the host they were built for: the channel publishes
# linux-amd64 while the control-plane runner is a Mac. Without a way across, the top
# tier would be unrecoverable exactly where the gate runs, and the baseline would sit
# at a lower tier describing an artifact the channel has already superseded. So a
# Linux release is read inside a container -- the same move deploy.yml already makes
# to read the candidate surface out of the release image. A darwin release has no
# such shim: it runs on a Mac or it is not read here.
CONTAINER_PLATFORMS = frozenset({"linux-amd64"})
CONTAINER = "docker"
# The runtime stage of stado-rs/Dockerfile. The published bytes are linked against
# this image's libc, so another base would measure that mismatch and call it a surface.
CONTAINER_IMAGE = "debian:bookworm-slim"

# Which machine a container must offer is asked of the bytes, not of the object key
# they were filed under. The key is a label an operator chose and this channel already
# serves one that disagrees with its own payload; the loader is not fooled by a label,
# so neither is this. ELF states the machine in a fixed little-endian field at 0x12,
# after declaring its own byte order at 0x05.
ELF_MAGIC = b"\x7fELF"
ELF_LITTLE = 1
ELF_DATA = 5
ELF_MACHINE = 18
ELF_MACHINE_WIDTH = ONE + ONE
ELF_MACHINES = {0x3E: "linux/amd64", 0xB7: "linux/arm64"}


class Unreachable(SystemExit):
    """A tier that exists but cannot be recovered here. Never degrade past it."""


def run(command: list, **extra) -> str:
    """One command's stdout, or a refusal naming what failed."""
    finished = subprocess.run(
        command, capture_output=True, text=True, check=False, **extra
    )
    if finished.returncode != OK:
        raise Unreachable(
            f"{' '.join(command)} exited {finished.returncode}: {finished.stderr.strip()}"
        )
    return finished.stdout


def native_platform():
    """The release platform this machine executes directly, if it is one at all."""
    return PLATFORMS.get((platform.system(), platform.machine()))


def readable_platform(available) -> str:
    """A published platform whose bytes can be executed here, native first.

    Native execution is preferred because it borrows nothing. The remaining tiers are
    ordered among themselves deterministically, so two hosts of the same shape never
    disagree about which published artifact the baseline describes.
    """
    for name in [native_platform(), *sorted(CONTAINER_PLATFORMS)]:
        if name in available:
            return name
    raise Unreachable(
        f"{PRODUCT} is published for {sorted(available)}, none of which can run "
        "here; a published surface is read from the bytes that run, so regenerate "
        f"the baseline on a {sorted(available)[OK]} host"
    )


def assert_refs_visible() -> None:
    """Refuse to rank tiers from a clone that cannot see the refs it is ranking.

    `actions/checkout` produces a shallow, tagless clone, and such a clone answers
    `git tag --list` with nothing however many tags the remote holds. A tier ranked
    there is not evidence: it would call the last-resort tier the best one and pass,
    blind to exactly the tag it exists to notice. Shallowness breaks `git archive
    <tag>` separately, because the tag's tree is simply absent.

    Visibility is settled against the remote, not against local config: `--no-tags` in
    a clone's config means future fetches skip tags, not that the tags fetched since
    are missing, so refusing on config alone would refuse a clone that can see
    everything. `git ls-remote` asks the only party that knows.
    """
    if run(["git", "rev-parse", "--is-shallow-repository"], cwd=REPOSITORY).strip() == "true":
        raise Unreachable(
            "this clone is shallow, so tag trees are absent and no tier can be "
            "recovered; run: git fetch --force --tags --unshallow"
        )
    if run(["git", "tag", "--list"], cwd=REPOSITORY).split():
        return
    if run(["git", "ls-remote", "--tags", "origin"], cwd=REPOSITORY).split():
        raise Unreachable(
            "the remote publishes tags this clone cannot see, so the tier ranking "
            "would be blind; run: git fetch --force --tags"
        )


def declared_version(tree: pathlib.Path) -> str:
    """The version one source tree declares in its crate manifest."""
    for line in (tree / MANIFEST).read_text(encoding="utf-8").splitlines():
        if line.startswith("version = "):
            return line.split('"')[ONE]
    raise Unreachable(f"{tree / MANIFEST} declares no version")


def as_triple(version: str) -> tuple:
    """A version as an orderable triple, or nothing if it is not one."""
    parts = version.split(".")
    if len(parts) != TRIPLE or not all(part.isdigit() for part in parts):
        return ()
    return tuple(int(part) for part in parts)


STATED = ("present", "absent")


def assert_channel_readable(stado: str) -> None:
    """Refuse to read silence as absence.

    An empty listing and an unreachable store are the same silence, and the wrong
    answer is the one that passes: the baseline would claim nothing is published
    because it failed to ask. `stat` on a full stado:// URI is the control, because it
    names one of three states for one object where the listing offers only silence. The
    probe object need not exist; a bare path would answer about the queue store instead
    and report absence forever.

    Only a stated present or absent counts. Unreachable, a missing field, or a state
    this script does not know is an answer nobody gave.
    """
    probe = (
        f"stado://{NAMESPACE}/{PRODUCT}/{declared_version(REPOSITORY)}"
        f"/{PROBE_PLATFORM}/{BINARY}"
    )
    state = channel_state(stado, probe)
    if state not in STATED:
        raise Unreachable(
            f"the release channel did not testify about {probe} (state {state!r}), so "
            "the absence of a published release cannot be established; refusing rather "
            "than assuming it either way"
        )


def channel_state(stado: str, uri: str) -> str:
    """What the channel says about one exact object: present, absent, or neither.

    Neither is not absence. Anything outside `STATED` is an answer nobody gave, and
    every caller here treats it as a reason to refuse rather than a reason to conclude.
    """
    return json.loads(run([stado, "storage", "stat", uri, "--json"])).get("state")


def published(stado: str) -> dict:
    """Published versions of this product mapped to the platforms they carry.

    The channel serves one object per request and offers no listing: releases are
    handed to its own route, which answers a served object by redirecting to where the
    bytes live. Enumerating `releases` through the object API used to stand in for
    that, and it quietly stopped being possible -- that route does not exist on the
    channel, so the call answered 404 for every caller. Under the gate's `set -e` the
    whole check died before judging anything; read any other way it would have meant
    "nothing is published", which is the one wrong answer that certifies a stale
    baseline.

    So candidates come from the tags this repository can see, and whether each is
    actually published comes from the channel, one exact coordinate at a time. A tag
    nobody released contributes nothing, which keeps the channel tier strictly above
    the tag tier instead of collapsing the two into "someone tagged it".
    """
    versions: dict = {}
    for version in tags_by_version():
        for platform_name in sorted(set(PLATFORMS.values())):
            key = f"{PRODUCT}/{version}/{platform_name}/{BINARY}"
            state = channel_state(stado, f"stado://{NAMESPACE}/{key}")
            if state not in STATED:
                raise Unreachable(
                    f"the release channel did not testify about {key} (state "
                    f"{state!r}), so what it publishes cannot be established; refusing "
                    "rather than assuming it either way"
                )
            if state == "present":
                versions.setdefault(version, {}).setdefault(platform_name, set()).add(
                    BINARY
                )
    if not versions:
        assert_channel_readable(stado)
    return versions


def container_platform(binary: pathlib.Path) -> str:
    """The container platform the published bytes themselves ask to run on."""
    with binary.open("rb") as handle:
        header = handle.read(ELF_MACHINE + ELF_MACHINE_WIDTH)
    if not header.startswith(ELF_MAGIC):
        raise Unreachable(f"{binary} is not an ELF executable, so no container runs it")
    if header[ELF_DATA] != ELF_LITTLE:
        raise Unreachable(f"{binary} is big-endian ELF, which no release platform is")
    machine = int.from_bytes(header[ELF_MACHINE:], "little")
    if machine not in ELF_MACHINES:
        raise Unreachable(
            f"{binary} needs ELF machine {machine:#x}, which is none of "
            f"{sorted(ELF_MACHINES.values())}"
        )
    return ELF_MACHINES[machine]


def container_help(binary: pathlib.Path, platform_name: str) -> str:
    """What a published binary prints on a host that cannot execute it directly.

    The image supplies a loader and nothing else: the binary is mounted read-only and
    replaces the entrypoint, so the answer still comes from the published bytes and
    not from anything the image ships under the same name. No network is offered,
    because a help screen that needed one would not be a help screen.
    """
    if shutil.which(CONTAINER) is None:
        raise Unreachable(
            f"{platform_name} does not run on this host and {CONTAINER} is not "
            f"installed, so the published {platform_name} surface cannot be read here"
        )
    wanted = container_platform(binary)
    if wanted != platform_name.replace("-", "/", ONE):
        print(
            f"note: {PRODUCT} is published under {platform_name} but its bytes are "
            f"{wanted}; reading the surface from what actually runs",
            file=sys.stderr,
        )
    return run(
        [
            CONTAINER,
            "run",
            "--rm",
            "--platform",
            wanted,
            "--network",
            "none",
            "--volume",
            f"{binary}:/{BINARY}:ro",
            "--entrypoint",
            f"/{BINARY}",
            CONTAINER_IMAGE,
            "help",
        ]
    )


def channel_surface(stado: str, version: str, platform_name: str) -> list:
    """The surface of a published release, read from the published bytes."""
    uri = f"stado://{NAMESPACE}/{PRODUCT}/{version}/{platform_name}/{BINARY}"
    with tempfile.TemporaryDirectory() as scratch:
        local = pathlib.Path(scratch) / BINARY
        run([stado, "storage", "get", uri, str(local)])
        executable = stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH
        local.chmod(local.stat().st_mode | executable)
        if platform_name == native_platform():
            return advertised(help_text(local))
        return advertised(container_help(local, platform_name))


def build_surface(ref: str) -> list:
    """The surface of a committed tree: export it, build it, ask the binary.

    A Rust command list cannot be read statically — it is what the built binary
    prints — so the tree has to be built. `--locked` is what keeps the answer a
    property of the commit rather than of this machine: the archive carries that
    commit's own Cargo.lock, and the build fails rather than resolving a dependency to
    whatever the local registry cache happens to hold.
    """
    target = pathlib.Path(os.environ.get("CARGO_TARGET_DIR") or DEFAULT_TARGET_CACHE)
    with tempfile.TemporaryDirectory() as scratch:
        tree = pathlib.Path(scratch)
        archive = tree / "tree.tar"
        with archive.open("wb") as handle:
            finished = subprocess.run(
                ["git", "archive", "--format=tar", ref],
                cwd=REPOSITORY,
                stdout=handle,
                stderr=subprocess.PIPE,
                text=True,
                check=False,
            )
        if finished.returncode != OK:
            raise Unreachable(f"git archive {ref} failed: {finished.stderr.strip()}")
        run(["tar", "-xf", str(archive)], cwd=tree)
        crate = tree / MANIFEST.parent
        environment = dict(os.environ, CARGO_TARGET_DIR=str(target))
        run(
            ["cargo", "build", "--release", "--locked", "--bin", BINARY],
            cwd=crate,
            env=environment,
        )
        return advertised(help_text(target / "release" / BINARY))


def tags_by_version() -> dict:
    """Tags whose own manifest declares a version triple, keyed by that version.

    The tag is trusted for nothing but its content: a tag whose tree declares a
    different version than its name suggests is filed under the version it actually
    contains, so a mis-signed tag cannot become a baseline for a version it does not
    hold.
    """
    found: dict = {}
    for tag in run(["git", "tag", "--list"], cwd=REPOSITORY).split():
        try:
            manifest = run(["git", "show", f"{tag}:{MANIFEST}"], cwd=REPOSITORY)
        except Unreachable:
            continue
        for line in manifest.splitlines():
            if line.startswith("version = "):
                version = line.split('"')[ONE]
                if not as_triple(version):
                    break
                if tag.lstrip("v") != version:
                    print(
                        f"tag {tag} declares {version}; filing it under {version}",
                        file=sys.stderr,
                    )
                found[version] = tag
                break
    return found


def newest(versions) -> str:
    """The greatest version triple in a collection."""
    return max(versions, key=as_triple)


def best_identity(stado: str) -> str:
    """The best artifact reachable right now, as a comparable identity.

    Deliberately recovers no surface: the gate calls this to notice that a baseline
    has been superseded — a tag, a newer release, or a first publication appearing
    after the baseline was generated leaves an honest marker measuring a stale
    artifact. Recomputing the surface here instead would let a regenerated surface
    reach the decision, which is the one shape that structurally cannot refuse.

    A published release is identified by its version, not by its object key: the key
    names one platform, and the same committed baseline is checked from hosts of
    different platforms. The last-resort tier is identified by its name alone, because
    a head sha moves with every commit and comparing it would demand a regenerated
    baseline per commit, forever.
    """
    assert_refs_visible()
    releases = published(stado)
    if releases:
        return f"{MARKER_CHANNEL}{newest(releases)}"
    tags = tags_by_version()
    if tags:
        return f"{MARKER_TAG}{tags[newest(tags)]}"
    return MARKER_HEAD.rstrip(":")


def document(stado: str) -> dict:
    """The baseline, from the best tier that actually exists."""
    assert_refs_visible()
    releases = published(stado)
    if releases:
        version = newest(releases)
        platform_name = readable_platform(releases[version])
        key = f"{PRODUCT}/{version}/{platform_name}/{BINARY}"
        return {
            "version": version,
            "source": f"{MARKER_CHANNEL}{key} release object published to the Stado channel",
            "surface": channel_surface(stado, version, platform_name),
        }

    tags = tags_by_version()
    if tags:
        version = newest(tags)
        tag = tags[version]
        return {
            "version": version,
            "source": f"{MARKER_TAG}{tag} rebuilt from the tag, which declares {version}",
            "surface": build_surface(tag),
        }

    head = run(["git", "rev-parse", "HEAD"], cwd=REPOSITORY).strip()
    return {
        "version": declared_version(REPOSITORY),
        "source": (
            f"{MARKER_HEAD}{head} the reachable Stado release channel serves no "
            "stado object and the repository has no tags, so the baseline is this "
            "commit's own build"
        ),
        "surface": build_surface(head),
    }


def main(argv: list) -> int:
    parser = argparse.ArgumentParser(
        description="Regenerate released-surface.json from the best available tier."
    )
    parser.add_argument(
        "--stado",
        default=os.environ.get("STADO_BIN") or str(pathlib.Path.home() / ".stado/bin/stado"),
        help="stado binary used to query the release channel",
    )
    parser.add_argument(
        "--output",
        default=str(REPOSITORY / BASELINE),
        help=f"where to write the baseline (default {BASELINE})",
    )
    parser.add_argument(
        "--print",
        dest="only_print",
        action="store_true",
        help="print the document instead of writing it",
    )
    parser.add_argument(
        "--best",
        dest="only_best",
        action="store_true",
        help="print the best reachable artifact identity and nothing else; recovers no surface",
    )
    args = parser.parse_args(argv[ONE:])

    if shutil.which(args.stado) is None and not pathlib.Path(args.stado).is_file():
        raise Unreachable(f"{args.stado} is not an executable stado")

    if args.only_best:
        print(best_identity(args.stado))
        return OK

    baseline = document(args.stado)
    if not baseline["surface"]:
        raise Unreachable("recovered an empty surface; refusing to write a baseline")

    rendered = json.dumps(baseline, indent=INDENT) + "\n"
    if args.only_print:
        sys.stdout.write(rendered)
        return OK
    pathlib.Path(args.output).write_text(rendered, encoding="utf-8")
    print(f"{args.output}: {baseline['version']} from {baseline['source'].split()[OK]}")
    return OK


if __name__ == "__main__":
    sys.exit(main(sys.argv))
