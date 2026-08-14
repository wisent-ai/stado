#!/usr/bin/env python3
"""Re-verify and, if needed, reinstall this host's Weles Chromium release.

The browser stopped rendering: every launch shape -- headless, headed,
screenshot -- ends in SIGSEGV or a hang, while the same build drove a full
Google login ninety minutes earlier. The host was at 2.7 GiB free while those
runs were writing gigabyte instrumentation dumps, and a bundle truncated by a
full disk looks exactly like this.

The release is a checksummed Stado artifact, so the repair is the installer the
deployment already uses: it verifies the receipt and re-downloads on mismatch.
Runs with the worker environment, because the version and digest live there.
"""

import os
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
TREE = HOME / "weles"
INSTALLER = TREE / "scripts" / "chromium" / "download.sh"
ENV_FILE = HOME / ".config" / "weles" / "worker.env"
TIMEOUT = float(len("s" * 900))


def worker_env():
    values = {}
    if not ENV_FILE.is_file():
        return values
    for line in ENV_FILE.read_text(encoding="utf-8", errors="replace").splitlines():
        stripped = line.strip().removeprefix("export ").strip()
        name, separator, raw = stripped.partition("=")
        if separator and not stripped.startswith("#"):
            # The launcher sources this file with `set -a`, so a value written
            # as `$HOME/...` is expanded by the shell. Reading it without
            # expanding hands the installer a literal dollar sign and it
            # refuses, which reads as a misconfigured host.
            value = raw.strip().strip('"').strip("'")
            values[name.strip()] = os.path.expandvars(os.path.expanduser(value))
    return values


def main():
    if not INSTALLER.is_file():
        raise SystemExit(f"no installer at {INSTALLER}")
    settings = worker_env()
    named = {
        key: settings.get(key, "(unset)")
        for key in ("WELES_CHROMIUM_RELEASE_VERSION", "STADO_RELEASE_API_URL", "STADO_RELEASE_LOCAL_ROOT")
    }
    print(f"coordinates {named}")
    # The receipt covers the archive, not the extracted tree: a bundle damaged
    # after extraction still verifies. `--force` re-extracts from the release,
    # which is the repair when the browser dies at launch while its receipt
    # says everything is fine.
    proc = subprocess.run(
        ["/bin/bash", str(INSTALLER), "--force"],
        capture_output=True,
        text=True,
        check=False,
        cwd=str(TREE),
        timeout=TIMEOUT,
        env={
            **os.environ,
            **settings,
            "HOME": str(HOME),
            "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin",
        },
    )
    print(f"exit        {proc.returncode}")
    print(f"binary      {proc.stdout.strip()[: len('a' * 160)] or '(none)'}")
    for line in (proc.stderr or "").splitlines()[-len("llllll"):]:
        if line.strip():
            print(f"  {line[: len('a' * 180)]}")
    return NONE


sys.exit(main())
