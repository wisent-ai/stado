#!/usr/bin/env python3
"""Install a locally built Stado program over the installed one, or refuse.

The operator's view is served by the binary this host runs, so a change to what
`overview` reports is invisible until that binary moves. Moving it is also the
riskiest edit on the machine: the resolver, the host agent, the dashboard and
every CLI call are the same executable.

So the swap is gated on the new binary answering the same questions as the old
one before it replaces anything: the version must not go backwards, and two
read-only commands that traverse the whole control plane must answer
byte-identically. Where the OLD binary fails a probe the new one passes, that is
recorded as a repair and allowed: agreement with a broken binary is not a safety
property, and a gate that demanded it could never replace the broken binary at
all. The previous binary is kept beside the new one under its version and the
date, which is the convention the vault binary already uses, and one `cp` puts it
back.

The program is the first argument, defaulting to `stado` so the no-argument
`stado host run-helper` invocation keeps its meaning. `stado_fleet` is here
because it is the program that enrolls machines and holds the fleet's SSH keys,
and it had no install path of its own — which is how this host ended up running
`stado_fleet` 0.5.1 against `stado` 0.7.2 until `key ls` started answering
HTTP 400.

Every program lands in the single owner-only bin directory, and `~/.local/bin`
carries a symlink to it. That is already how `stado` is wired; a second real
binary somewhere on PATH is how versions diverge unobserved.

Nothing here restarts a service. Restarts are a separate, observed step.
"""

import datetime
import hashlib
import os
import pathlib
import shutil
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
PROGRAMS = {
    # program -> read-only commands whose answers must match. Both of `stado`'s
    # traverse the whole control plane; `stado_fleet`'s read the same registry
    # through its own enrollment and catalog paths.
    "stado": (("registry", "pull"), ("registry", "self")),
    "stado_fleet": (("list",), ("catalog",)),
}
NAME = sys.argv[len("a")] if len(sys.argv) > len("a") else "stado"
INSTALLED = HOME / ".stado" / "bin" / NAME
LINK = HOME / ".local" / "bin" / NAME
# Where the candidate comes from: an explicit override, this host's own build,
# or an artifact delivered by `stado host install-file`. A host with no Rust
# toolchain still has to be able to receive a checked binary.
CANDIDATES = (
    pathlib.Path(os.environ.get("STADO_BUILT_BINARY", "")),
    HOME / ".cache" / "stado-build" / "debug" / NAME,
    HOME / ".stado" / "files" / f"{NAME}-incoming",
)
BUILT = next((path for path in CANDIDATES if str(path) and path.is_file()), CANDIDATES[len("a")])
TIMEOUT = 300


def version_of(binary):
    printed = subprocess.run(
        [str(binary), "--version"], capture_output=True, text=True, check=False, timeout=TIMEOUT
    ).stdout.strip()
    tail = printed.split()[-1:] if printed else []
    return tail[ZERO] if tail else "unknown"


def ordered(version):
    """Sortable form of a dotted version, so 0.7.2 is not 'less than' 0.10.0."""
    parts = []
    for piece in version.split("."):
        digits = "".join(character for character in piece if character.isdigit())
        parts.append(int(digits) if digits else ZERO)
    return tuple(parts)


def digest(path):
    reader = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1 << len("aaaaaaaaaaaaaaaaaaaa")), b""):
            reader.update(chunk)
    return reader.hexdigest()[: len("a" * 16)]


def answer(binary, argv):
    proc = subprocess.run(
        [str(binary), *argv], capture_output=True, text=True, check=False, timeout=TIMEOUT
    )
    return proc.returncode, hashlib.sha256(proc.stdout.encode()).hexdigest()[: len("a" * 16)]


def link_to(installed, link):
    """Point ~/.local/bin at the owner-only binary, atomically."""
    if link.is_symlink() and link.resolve() == installed:
        print(f"link       {link} -> {installed}")
        return
    link.parent.mkdir(parents=True, exist_ok=True)
    staged = link.with_name(f"{link.name}.link-{os.getpid()}")
    if staged.exists() or staged.is_symlink():
        staged.unlink()
    os.symlink(installed, staged)
    os.replace(staged, link)
    print(f"link       {link} -> {installed} (rewritten)")


def main():
    if NAME not in PROGRAMS:
        raise SystemExit(f"unknown program {NAME!r}; one of {', '.join(PROGRAMS)}")
    if not BUILT.is_file():
        raise SystemExit(f"no built binary at {BUILT}")
    # A program installed outside the owner-only bin directory is the same
    # program with no version story: adopt it there before comparing, so the
    # copy this script gates is the copy PATH resolves.
    if not INSTALLED.is_file() and LINK.is_file() and not LINK.is_symlink():
        INSTALLED.parent.mkdir(parents=True, exist_ok=True)
        shutil.move(str(LINK), str(INSTALLED))
        print(f"adopted    {LINK} -> {INSTALLED}")
    prior = INSTALLED.is_file()
    # A binary delivered by `stado host install-file` lands owner-read-only,
    # which is right for a secret and wrong for something that has to answer
    # questions before it is trusted. Stage an executable copy first and probe
    # that: the candidate is never run from the delivery path, and the delivery
    # path is never made executable in place.
    staged = INSTALLED.with_name(f"{NAME}.incoming-{os.getpid()}")
    shutil.copy2(BUILT, staged)
    os.chmod(staged, 0o755)
    running = version_of(INSTALLED) if prior else "absent"
    incoming = version_of(staged)
    print(f"installed  {INSTALLED} {running}" + (f" sha256 {digest(INSTALLED)}" if prior else ""))
    print(f"candidate  {BUILT} {incoming} sha256 {digest(staged)}")
    if prior and ordered(incoming) < ordered(running):
        # The source tree can lag what production runs; installing then quietly
        # removes whatever the newer line carried. Bump the source version
        # instead of stepping backwards.
        staged.unlink()
        raise SystemExit(
            f"refusing to install: {incoming} is older than the running {running}"
        )
    disagreed, unanswered = [], []
    for argv in PROGRAMS[NAME]:
        old = answer(INSTALLED, argv) if prior else NONE
        new = answer(staged, argv)
        if new[ZERO] != ZERO:
            verdict = "NEW FAILS"
            unanswered.append(" ".join(argv))
        elif old is NONE:
            verdict = "first install"
        elif old == new:
            verdict = "agree"
        elif old[ZERO] != ZERO:
            # The running binary cannot answer this at all. Requiring the
            # replacement to reproduce that failure would make the gate
            # unopenable by the only thing that fixes it.
            verdict = "repairs"
        else:
            verdict = "DISAGREE"
            disagreed.append(" ".join(argv))
        shown = f"old exit {old[ZERO]} {old[1]}" if old else "old absent"
        print(f"  {' '.join(argv):16} {shown}  new exit {new[ZERO]} {new[1]}  {verdict}")
    if unanswered or disagreed:
        staged.unlink()
        raise SystemExit(
            f"refusing to install: new binary fails {unanswered} and answers differently for {disagreed}"
        )
    restore = NONE
    if prior:
        stamp = datetime.datetime.now().strftime("%Y%m%d")
        backup = INSTALLED.with_name(f"{NAME}.{running}-backup-{stamp}")
        if not backup.exists():
            shutil.copy2(INSTALLED, backup)
        print(f"backup     {backup} sha256 {digest(backup)}")
        restore = backup
    os.replace(staged, INSTALLED)
    print(f"installed  {INSTALLED} sha256 {digest(INSTALLED)}")
    link_to(INSTALLED, LINK)
    if restore:
        print("restore    cp " + str(restore) + " " + str(INSTALLED))
    return NONE


sys.exit(main())
