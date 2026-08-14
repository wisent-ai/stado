#!/usr/bin/env python3
"""Run one headless launch, then read the report that launch produced.

The crash reports already on this host were left by headed runs, so they name
window-server teardown and prove nothing about `--headless=new`. This runs the
headless launch alone, notes the time, and reads only reports written after it,
so the stack belongs to the launch and not to a neighbour.
"""

import datetime
import json
import os
import pathlib
import subprocess
import sys
import time

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
REPORTS = HOME / "Library" / "Logs" / "DiagnosticReports"
FRAMES = 16
TIMEOUT = 90
PAGE = "data:text/html,<title>probe</title><h1>probe</h1>"


def newest_binary():
    root = HOME / ".local" / "share" / "weles-chromium"
    found = sorted(root.glob("*/Chromium.app/Contents/MacOS/Chromium"))
    if not found:
        raise SystemExit("no Weles Chromium on this host")
    return found[-1]


def launch(binary, mode, extra):
    started = time.time()
    try:
        proc = subprocess.run(
            [str(binary), *extra, "--no-sandbox", "--disable-gpu", "--user-data-dir=" + str(HOME / ".local" / "state" / "weles" / f"headless-probe-{mode}"), PAGE],
            capture_output=True,
            text=True,
            check=False,
            timeout=TIMEOUT,
        )
        code, out, err = proc.returncode, proc.stdout, proc.stderr
    except subprocess.TimeoutExpired as expired:
        code, out, err = "timed out", str(expired.stdout or ""), str(expired.stderr or "")
    print(f"== {mode}")
    print(f"   exit {code} stdout {len(out)} bytes")
    for line in (err or "").splitlines()[-6:]:
        if line.strip():
            print(f"   {line[: len('a' * 150)]}")
    return started


def report_after(started):
    if not REPORTS.is_dir():
        print("   no report directory")
        return NONE
    fresh = [path for path in REPORTS.glob("Chromium*.ips") if path.stat().st_mtime >= started - 1]
    if not fresh:
        print("   no crash report written by this launch")
        return NONE
    path = max(fresh, key=lambda entry: entry.stat().st_mtime)
    body = path.read_text(encoding="utf-8", errors="replace").partition("\n")[2]
    try:
        report = json.loads(body)
    except json.JSONDecodeError:
        print(f"   unparseable {path.name}")
        return NONE
    exception = report.get("exception", {})
    print(f"   report {path.name}")
    print(f"   exception {exception.get('type')} {exception.get('signal')} {exception.get('subtype')}")
    images = report.get("usedImages", [])
    thread = next((entry for entry in report.get("threads", []) if entry.get("triggered")), {})
    print(f"   thread {thread.get('name') or '(unnamed)'}")
    for frame in thread.get("frames", [])[:FRAMES]:
        index = frame.get("imageIndex", ZERO)
        image = images[index] if index < len(images) else {}
        print(f"     {image.get('name', '?'):30} {frame.get('symbol') or '+' + str(frame.get('imageOffset'))}")
    return NONE


def main():
    binary = newest_binary()
    print(f"binary {binary}")
    print(f"now    {datetime.datetime.now().isoformat(timespec='seconds')}")
    for mode, extra in (
        ("headless-new-dump", ["--headless=new", "--dump-dom"]),
        ("headless-old-dump", ["--headless=old", "--dump-dom"]),
        ("version-only", ["--version"]),
    ):
        started = launch(binary, mode, extra)
        report_after(started)
    return NONE


sys.exit(main())
