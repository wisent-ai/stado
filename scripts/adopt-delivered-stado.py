#!/usr/bin/env python3
"""Move a delivered Stado binary into place on this host, keeping the old one.

`host install-file` lands a delivered artifact under `~/.stado/files` with no
execute bit, which is the right default: delivery and adoption are different
decisions. This performs the adoption, and only that.

The installed binary is copied aside before it is replaced, so the previous
control plane stays one copy away. The delivered file must report a Stado
version before anything is moved -- finding out that it cannot after the move is
finding out too late.

Usage: adopt-delivered-stado.py   (adopts $HOME/.stado/files/stado-next.bin)
"""
import datetime
import os
import pathlib
import shutil
import stat
import subprocess
import sys

OWNER_EXEC = stat.S_IRWXU
OWNER_EXEC_GROUP_READ = stat.S_IRWXU | stat.S_IRGRP | stat.S_IXGRP
FIRST = len(["first"])
NONE = len([])
HOME = pathlib.Path.home()

delivered = pathlib.Path(os.environ.get("STADO_DELIVERED", HOME / ".stado" / "files" / "stado-next.bin"))
installed = pathlib.Path(os.environ.get("STADO_INSTALLED", HOME / ".stado" / "bin" / "stado"))


def version_of(binary):
    try:
        result = subprocess.run(
            [str(binary), "--version"], capture_output=True, text=True, check=False, timeout=float(len("aaaaaaaaaa"))
        )
    except OSError as error:
        return f"(unrunnable: {error})"
    output = (result.stdout or result.stderr).strip().splitlines()
    return output[NONE] if output else "(silent)"


def main():
    if not delivered.is_file():
        raise SystemExit(f"no delivered binary at {delivered}")

    staging = installed.with_name(f".{installed.name}.adopt-{os.getpid()}")
    shutil.copyfile(delivered, staging)
    os.chmod(staging, OWNER_EXEC)

    reported = version_of(staging)
    if not reported.startswith("stado"):
        staging.unlink(missing_ok=True)
        raise SystemExit(f"delivered binary does not report a stado version: {reported}")

    previous = "(absent)"
    if installed.exists():
        previous = version_of(installed)
        stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
        backup = installed.with_name(f"{installed.name}.before-adopt-{stamp}")
        shutil.copy2(installed, backup)
        print(f"backup    {backup}")

    staging.replace(installed)
    os.chmod(installed, OWNER_EXEC_GROUP_READ)

    print(f"previous  {previous}")
    print(f"installed {version_of(installed)}")
    print(f"path      {installed}")
    return NONE


sys.exit(main())
