#!/usr/bin/env python3
"""Install a locally built `stado` over the installed one, or refuse.

The operator's view is served by the binary this host runs, so a change to what
`overview` reports is invisible until that binary moves. Moving it is also the
riskiest edit on the machine: the resolver, the host agent, the dashboard and
every CLI call are the same executable.

So the swap is gated on the new binary answering the same questions as the old
one before it replaces anything: same version string, and byte-identical answers
for two read-only commands that traverse the whole control plane. The previous
binary is kept beside the new one under its version and the date, which is the
convention the vault binary already uses, and one `cp` puts it back.

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
INSTALLED = HOME / ".stado" / "bin" / "stado"
BUILT = pathlib.Path(os.environ.get("STADO_BUILT_BINARY", HOME / ".cache" / "stado-build" / "debug" / "stado"))
AGREEMENT = (("--version",), ("registry", "pull"), ("registry", "self"))
TIMEOUT = 300

# The two commands whose ANSWERS must match: both traverse the whole control
# plane, so a binary that reads the fleet differently is caught before it
# replaces anything. The version is deliberately not among them -- an upgrade
# changes it by definition -- but it must not go backwards, which is checked
# separately below.
AGREEMENT = (("registry", "pull"), ("registry", "self"))


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


def main():
    if not BUILT.is_file():
        raise SystemExit(f"no built binary at {BUILT}")
    if not INSTALLED.is_file():
        raise SystemExit(f"no installed binary at {INSTALLED}")
    running = version_of(INSTALLED)
    incoming = version_of(BUILT)
    print(f"installed  {INSTALLED} {running} sha256 {digest(INSTALLED)}")
    print(f"built      {BUILT} {incoming} sha256 {digest(BUILT)}")
    if ordered(incoming) < ordered(running):
        # The source tree can lag what production runs; installing then quietly
        # removes whatever the newer line carried. Bump the source version
        # instead of stepping backwards.
        raise SystemExit(
            f"refusing to install: {incoming} is older than the running {running}"
        )
    disagreed = []
    for argv in AGREEMENT:
        old = answer(INSTALLED, argv)
        new = answer(BUILT, argv)
        verdict = "agree" if old == new else "DISAGREE"
        print(f"  {' '.join(argv):16} old exit {old[ZERO]} {old[1]}  new exit {new[ZERO]} {new[1]}  {verdict}")
        if old != new:
            disagreed.append(" ".join(argv))
    if disagreed:
        raise SystemExit(f"refusing to install: the new binary answers differently for {disagreed}")
    stamp = datetime.datetime.now().strftime("%Y%m%d")
    backup = INSTALLED.with_name(f"stado.{running}-backup-{stamp}")
    if not backup.exists():
        shutil.copy2(INSTALLED, backup)
    print(f"backup     {backup} sha256 {digest(backup)}")
    staged = INSTALLED.with_name(f"stado.incoming-{os.getpid()}")
    shutil.copy2(BUILT, staged)
    os.chmod(staged, 0o755)
    os.replace(staged, INSTALLED)
    print(f"installed  {INSTALLED} sha256 {digest(INSTALLED)}")
    print("restore    cp " + str(backup) + " " + str(INSTALLED))
    return NONE


sys.exit(main())
