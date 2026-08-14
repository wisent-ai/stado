#!/usr/bin/env python3
"""Ask whether any browser on this host can render, or only the patched one dies.

The Weles Chromium release segfaults here in every launch mode, including
`--headless=new`, which needs no window server at all. That is either a broken
build on this host or a host that no browser can start on, and the two call for
opposite repairs. A stock browser answers the question: if Playwright's own
Chromium renders a page headlessly while the patched release crashes, the fault
is the build, not the missing login session.

Prints exit codes and rendered byte counts only.
"""

import os
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
STATE = HOME / ".local" / "state" / "weles"
PAGE = "data:text/html,<title>probe</title><h1>probe</h1>"
TIMEOUT = 60


def candidates():
    found = []
    playwright = HOME / "Library" / "Caches" / "ms-playwright"
    if playwright.is_dir():
        for entry in sorted(playwright.glob("chromium*/chrome-mac/Chromium.app/Contents/MacOS/Chromium")):
            found.append(("playwright", entry))
    for path in (
        pathlib.Path("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
        pathlib.Path("/Applications/Chromium.app/Contents/MacOS/Chromium"),
    ):
        if path.is_file():
            found.append(("installed", path))
    weles = HOME / ".local" / "share" / "weles-chromium"
    if weles.is_dir():
        for entry in sorted(weles.glob("*/Chromium.app/Contents/MacOS/Chromium")):
            found.append(("weles", entry))
    return found


def render(binary, label):
    STATE.mkdir(parents=True, exist_ok=True)
    shot = STATE / f"stock-probe-{label}.png"
    if shot.is_file():
        shot.unlink()
    try:
        proc = subprocess.run(
            [
                str(binary),
                "--headless=new",
                "--no-sandbox",
                "--disable-gpu",
                f"--screenshot={shot}",
                "--window-size=400,300",
                PAGE,
            ],
            capture_output=True,
            text=True,
            check=False,
            timeout=TIMEOUT,
        )
        code = proc.returncode
    except subprocess.TimeoutExpired:
        code = "timed out"
    size = shot.stat().st_size if shot.is_file() else ZERO
    print(f"{label:12} exit {code} wrote {size} bytes  {binary}")
    return size


def main():
    found = candidates()
    if not found:
        print("no browser binaries on this host")
        return NONE
    rendered = [render(path, kind) for kind, path in found]
    print("verdict " + ("some browser renders headlessly" if any(rendered) else "no browser renders headlessly"))
    return NONE


sys.exit(main())
