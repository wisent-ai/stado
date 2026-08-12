#!/usr/bin/env python3
"""Install this host's Stado resolver as a managed system daemon.

The registry declares `service_resolver` for a host -- an API bind and the
stable-port adapters every co-located workload dials -- but nothing guarantees
the process that serves them is running. On a headless host that gap is
permanent: `service deploy` renders a LaunchAgent, and a machine with no console
session cannot bootstrap one ("could not switch to audit session"), so the
resolver ends up started by hand and dies at the next reboot. Every always-on
unit on such a host is a LaunchDaemon for exactly this reason.

This renders the daemon, installs it under /Library/LaunchDaemons with the
passwordless launchctl grant the fleet already relies on, and bootstraps it.
`stado service adopt` afterwards brings it under registry management.

Idempotent: an existing unit is booted out and replaced by the rendered one.
"""
import os
import pathlib
import plistlib
import socket
import subprocess
import sys

NONE = len([])
HOME = pathlib.Path.home()
LABEL = os.environ.get("STADO_RESOLVER_LABEL", "com.wisent.always-on.stado-resolver")
PLIST = pathlib.Path("/Library/LaunchDaemons") / f"{LABEL}.plist"
STADO = pathlib.Path(os.environ.get("STADO_BIN", HOME / ".stado" / "bin" / "stado"))
LOGS = HOME / ".stado" / "logs"


def run(*arguments, check=True):
    result = subprocess.run(list(arguments), capture_output=True, text=True, check=False)
    if check and result.returncode != NONE:
        detail = (result.stderr or result.stdout).strip()
        raise SystemExit(f"{' '.join(arguments)} failed: {detail}")
    return (result.stdout or "").strip()


def target_name():
    configured = os.environ.get("STADO_RESOLVER_TARGET", "").strip()
    if configured:
        return configured
    reported = run(str(STADO), "registry", "self", check=False)
    first = reported.split("\t")[NONE].strip() if reported else ""
    return first or socket.gethostname().split(".")[NONE].lower()


def main():
    if not STADO.is_file():
        raise SystemExit(f"no stado binary at {STADO}")
    LOGS.mkdir(parents=True, exist_ok=True)
    target = target_name()

    # launchd hands a daemon none of the login shell's environment, and the
    # resolver opens the registry store through it. Without the backend the
    # other always-on units declare, it opens a different store and rejects the
    # document it finds there -- which reads as a corrupt registry and is a
    # missing variable. The values mirror `com.wisent.always-on.stado-object-api`
    # on this host.
    environment = {
        "HOME": str(HOME),
        "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
    }
    for name in ("WC_STORAGE_BACKEND", "WC_BUCKET", "WC_SKARBIEC_URL", "STADO_RELEASE_PLATFORM"):
        value = os.environ.get(name, "").strip()
        if value:
            environment[name] = value

    document = {
        "Label": LABEL,
        "ProgramArguments": [str(STADO), "resolver", "serve", "--target", target],
        "RunAtLoad": True,
        "KeepAlive": True,
        "UserName": os.environ.get("USER", HOME.name),
        "WorkingDirectory": str(HOME),
        "EnvironmentVariables": environment,
        "StandardOutPath": str(LOGS / "stado-resolver.out"),
        "StandardErrorPath": str(LOGS / "stado-resolver.err"),
    }

    staging = HOME / ".stado" / "files" / f"{LABEL}.plist"
    staging.parent.mkdir(parents=True, exist_ok=True)
    with staging.open("wb") as handle:
        plistlib.dump(document, handle)

    run("/usr/bin/sudo", "-n", "/bin/cp", str(staging), str(PLIST))
    run("/usr/bin/sudo", "-n", "/usr/sbin/chown", "root:wheel", str(PLIST))
    run("/usr/bin/sudo", "-n", "/bin/chmod", "u=rw,go=r", str(PLIST))

    run("/usr/bin/sudo", "-n", "/bin/launchctl", "bootout", f"system/{LABEL}", check=False)
    run("/usr/bin/sudo", "-n", "/bin/launchctl", "enable", f"system/{LABEL}", check=False)
    run("/usr/bin/sudo", "-n", "/bin/launchctl", "bootstrap", "system", str(PLIST))

    print(f"label   {LABEL}")
    print(f"target  {target}")
    print(f"plist   {PLIST}")
    print(run("/usr/bin/sudo", "-n", "/bin/launchctl", "print", f"system/{LABEL}", check=False).splitlines()[NONE:len(["state"])])
    return NONE


sys.exit(main())
