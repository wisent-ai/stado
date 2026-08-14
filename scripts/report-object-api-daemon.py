#!/usr/bin/env python3
"""Name the launchd units that serve this host's object API, loopback and tailnet.

`stado service list` shows the registry-managed units, and the object gateway is
not one of them: the fleet reaches `stado://probierz/...` through a `stado
dashboard` process nobody's service table names. An operator who has to restart
that process to apply a policy change should not have to guess its label from a
pid, so this matches the port the registry declares for this host against the
program each system daemon runs and prints the one that answers.

The gateway binds loopback, so every off-host caller depends on a second unit:
the tailnet proxy that fronts it. A host whose proxy is not running looks, from
another machine, exactly like a host whose API is down -- which is why the Linux
member of this fleet reads its own empty disk instead of the fleet store. Both
units are reported together, because the answer "the API is up" is worth nothing
without "and it is reachable from where the caller stands".

Read-only: it reads plists and asks each endpoint for `/healthz`.
"""

import json
import os
import pathlib
import subprocess
import sys
import urllib.error
import urllib.request

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
STADO = HOME / ".stado" / "bin" / "stado"
DAEMONS = pathlib.Path("/Library/LaunchDaemons")
AGENTS = HOME / "Library" / "LaunchAgents"
SERVICE = "stado-object-api"
PROBE_TIMEOUT = len("aaaaa")
PROXY = HOME / ".stado" / "bin" / "stado-tailnet-object-proxy"


def run(*args):
    proc = subprocess.run(
        args, capture_output=True, text=True, check=False, timeout=len("a" * 60)
    )
    return proc.returncode, (proc.stdout + proc.stderr).strip()


def declared_url():
    code, output = run(str(STADO), "registry", "pull")
    if code != ZERO:
        raise SystemExit(f"stado registry pull failed: {output}")
    document = json.loads(output)
    service = document["service_directory"]["services"][SERVICE]
    node = os.uname().nodename.lower().split(".")[ZERO]
    for name, endpoint in service.get("endpoints", {}).items():
        target = next((entry for entry in document["targets"] if entry.get("name") == name), {})
        names = [str(value).lower() for value in target.get("hostnames", [])] + [name.lower()]
        if any(value.split(".")[ZERO] == node for value in names if value):
            return name, str(endpoint["url"])
    raise SystemExit(f"this host declares no {SERVICE} endpoint in the registry")


def units_matching(needle):
    """Every launchd unit, system or user, whose program contains this text."""
    found = []
    for domain, folder in (("system", DAEMONS), ("gui", AGENTS)):
        if not folder.is_dir():
            continue
        for plist in sorted(folder.glob("*.plist")):
            code, printed = run("/usr/bin/plutil", "-convert", "json", "-o", "-", str(plist))
            if code != ZERO:
                continue
            try:
                unit = json.loads(printed)
            except ValueError:
                continue
            arguments = " ".join(str(word) for word in unit.get("ProgramArguments", []))
            if needle in arguments:
                found.append((domain, unit.get("Label", plist.stem), arguments, plist))
    return found


def describe(units):
    for domain, label, arguments, plist in units:
        print(f"unit        {domain}/{label}")
        print(f"  plist     {plist}")
        print(f"  runs      {arguments[: len('a' * 140)]}")
        code, printed = run("/usr/bin/plutil", "-convert", "json", "-o", "-", str(plist))
        unit = json.loads(printed) if code == ZERO else {}
        for name, value in sorted(unit.get("EnvironmentVariables", {}).items()):
            # A path is the answer an operator needs when rebuilding this unit;
            # a bearer is never printed, only measured, because the whole point
            # of the owner-only files this fleet uses is that they stay unread.
            secret = any(word in name.upper() for word in ("TOKEN", "SECRET", "PASSWORD"))
            shown = f"[{len(str(value))} bytes withheld]" if secret else str(value)
            print(f"  env       {name}={shown}")
        for name in ("StandardOutPath", "StandardErrorPath", "UserName", "RunAtLoad", "KeepAlive"):
            if name in unit:
                print(f"  {name:<9} {unit[name]}")
        program = pathlib.Path(str(unit.get("ProgramArguments", [""])[ZERO]))
        if program.is_file() and program.stat().st_size < len("a" * 8000):
            for line in program.read_text("utf-8", "replace").splitlines():
                if line.strip() and not line.lstrip().startswith("#"):
                    print(f"  launcher  {line.strip()[: len('a' * 150)]}")
        code, printed = run("/usr/bin/sudo", "-n", "/bin/launchctl", "print", f"{domain}/{label}")
        state = [line.strip() for line in printed.splitlines() if line.strip().startswith("state")]
        print(f"  launchd   {state[ZERO] if state else printed.splitlines()[ZERO][: len('a' * 80)]}")
    if not units:
        print("unit        (no launchd unit names this program)")


def main():
    name, url = declared_url()
    port = url.rsplit(":", len("a"))[-1]
    print(f"target      {name}")
    print(f"endpoint    {url}")
    code, listeners = run("/usr/sbin/lsof", "-nP", f"-iTCP:{port}", "-sTCP:LISTEN")
    for line in listeners.splitlines()[len("a"):]:
        print(f"listener    {line[: len('a' * 120)]}")
    describe(units_matching(f"--port {port}"))
    print(f"proxy       {PROXY}  {'present' if PROXY.is_file() else 'absent'}")
    describe(units_matching(PROXY.name))
    code, addresses = run("/usr/sbin/ipconfig", "getifaddr", "utun4")
    tailnet = addresses.strip() if code == ZERO else NONE
    print(f"tailnet     {tailnet or '(no tailscale address on utun4)'}")
    try:
        with urllib.request.urlopen(f"{url.rstrip('/')}/healthz", timeout=PROBE_TIMEOUT) as answer:
            print(f"healthz     HTTP {answer.status} {answer.read().decode('utf-8', 'replace')[: len('a' * 120)]}")
    except (urllib.error.URLError, OSError) as error:
        print(f"healthz     unreachable: {error}")
    return NONE


sys.exit(main())
