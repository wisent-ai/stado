#!/usr/bin/env python3
"""Put the checked-in object-API launcher in place and restart the service.

The launcher decides where the fleet's object API listens, so a wrong copy takes
the store down for every reader on the host. It is delivered as an owner-only
file by `stado host install-file`, which is right for delivery and wrong for
execution, so it is copied to the executable path with the previous version kept
beside it, then the service is restarted and its listener verified.

Refuses to finish unless the service answers again.
"""

import datetime
import os
import pathlib
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.request

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
DELIVERED = HOME / ".stado" / "files" / "stado-object-api-launcher.sh"
INSTALLED = HOME / ".stado" / "bin" / "stado-object-api-launcher"
LABEL = os.environ.get(
    "OBJECT_API_LABEL", "com.wisent.compute.service.stado-object-api"
)
SETTLE = 40


def run(*args):
    proc = subprocess.run(args, capture_output=True, text=True, check=False)
    return proc.stdout + proc.stderr


def port_from_config():
    import json

    config = pathlib.Path(os.environ.get("STADO_CONFIG", HOME / ".config" / "stado" / "config.json"))
    document = json.loads(config.read_text(encoding="utf-8")) if config.is_file() else {}
    return (document.get("object_api") or {}).get("port") or 18765


def answers(port):
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/healthz", timeout=10) as answer:
            return answer.status
    except urllib.error.HTTPError as error:
        return error.code
    except OSError:
        return NONE


def main():
    if not DELIVERED.is_file():
        raise SystemExit(f"no delivered launcher at {DELIVERED}")
    port = port_from_config()
    print(f"port       {port} (from object_api.port, else the historical default)")
    if INSTALLED.is_file():
        stamp = datetime.datetime.now().strftime("%Y%m%dT%H%M%SZ")
        backup = INSTALLED.with_name(f"{INSTALLED.name}.before-{stamp}")
        shutil.copy2(INSTALLED, backup)
        print(f"backup     {backup.name}")
    shutil.copyfile(DELIVERED, INSTALLED)
    os.chmod(INSTALLED, 0o755)
    print(f"installed  {INSTALLED}")
    domain = f"gui/{os.getuid()}" if LABEL.startswith("com.wisent.compute") else "system"
    prefix = ["/usr/bin/sudo", "-n"] if domain == "system" else []
    print(f"restart    {run(*prefix, '/bin/launchctl', 'kickstart', '-k', f'{domain}/{LABEL}').strip() or 'accepted'}")
    deadline = time.time() + SETTLE
    while time.time() < deadline:
        if answers(port) == 200:
            break
        time.sleep(len("ab"))
    verdict = answers(port)
    print(f"healthz    {verdict}")
    if verdict != 200:
        raise SystemExit("the object API did not answer again; restore the backup before continuing")
    return NONE


sys.exit(main())
