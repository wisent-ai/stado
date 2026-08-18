#!/usr/bin/env python3
"""Give the declared media router the unit it never got.

`stado registry doctor` reported `missing-plist` for `image-video-router` on the
GPU host, and it was right: the release is deployed
(`~/.stado/services/image-video-router/releases/0.1.0/linux-amd64`), its runtime
directory and `service.env` exist, the registry names the unit
`image-video-router-release.service` and the service directory hands two
consumers -- `content-platform` and `weles` -- an endpoint on port 8081. Only the
systemd unit was never written, so nothing has listened there.

`stado service deploy` refuses while the registry still manages the name, and
`stado service retire` cannot remove the declaration while the directory entry
points at it, so neither half of that pair can run first. The declaration is not
the thing that is wrong here -- the missing unit is. This writes it, with exactly
the four values the release's `start` script requires Stado to pin, then enables
and starts it and reports whether the port answers.

Idempotent: an existing unit file with the same content is left alone.
"""

import json
import pathlib
import subprocess
import sys
import time

NONE = None
NAME = "image-video-router"
UNIT = pathlib.Path("/etc/systemd/system/image-video-router-release.service")
RELEASE = pathlib.Path(f"/root/.stado/services/{NAME}/releases/0.1.0/linux-amd64")
RUNTIME = pathlib.Path(f"/root/.stado/run/{NAME}")
ENV_FILE = pathlib.Path(f"/root/.config/{NAME}/service.env")
PORT = "8081"
SETTLE = 12

BODY = """[Unit]
Description=Wisent image-video-router (Stado-managed release {version})
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
Environment=IMAGE_VIDEO_ROUTER_BIN={binary}
Environment=IMAGE_VIDEO_ROUTER_PORT={port}
Environment=IMAGE_VIDEO_ROUTER_RUNTIME_DIR={runtime}
Environment=IMAGE_VIDEO_ROUTER_SERVICE_ENV_FILE={env_file}
ExecStart={start}
Restart=on-failure
RestartSec=5
WorkingDirectory={runtime}

[Install]
WantedBy=multi-user.target
"""


def run(*args):
    proc = subprocess.run(args, capture_output=True, text=True, check=False)
    return (proc.stdout + proc.stderr).strip()


def newest_runtime():
    if not RUNTIME.is_dir():
        return RUNTIME
    children = sorted((child for child in RUNTIME.iterdir() if child.is_dir()),
                      key=lambda child: child.name)
    return children[-1] if children else RUNTIME


def main():
    manifest_path = RELEASE / ".stado-release.json"
    for required in (manifest_path, RELEASE / "bin" / "start", ENV_FILE):
        if not required.exists():
            raise SystemExit(f"the release is incomplete: {required} is absent")
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    binary = RELEASE / manifest.get("binary", "bin/image-video-router")
    body = BODY.format(
        version=manifest.get("version", "unknown"),
        binary=binary,
        port=PORT,
        runtime=newest_runtime(),
        env_file=ENV_FILE,
        start=RELEASE / "bin" / "start",
    )
    print(f"unit       {UNIT}")
    if UNIT.is_file() and UNIT.read_text(encoding="utf-8") == body:
        print("settled    the unit already says this")
    else:
        staged = pathlib.Path("/tmp/image-video-router-release.service")
        staged.write_text(body, encoding="utf-8")
        print(f"write      {run('/usr/bin/sudo', '-n', '/bin/cp', str(staged), str(UNIT)) or 'ok'}")
        staged.unlink(missing_ok=True)
        run("/usr/bin/sudo", "-n", "/bin/systemctl", "daemon-reload")
    print(f"enable     {run('/usr/bin/sudo', '-n', '/bin/systemctl', 'enable', '--now', UNIT.name) or 'ok'}")
    time.sleep(SETTLE)
    active = run("/bin/systemctl", "is-active", UNIT.name)
    print(f"active     {active}")
    answered = run("/usr/bin/curl", "-s", "-m", "8", "-o", "/dev/null",
                   "-w", "%{http_code}", f"http://127.0.0.1:{PORT}/healthz")
    print(f"port {PORT}  http={answered or 'no answer'}")
    if active != "active":
        print("journal   " + run("/bin/journalctl", "-u", UNIT.name, "-n", "4", "--no-pager")[-300:])
    return NONE


sys.exit(main())
