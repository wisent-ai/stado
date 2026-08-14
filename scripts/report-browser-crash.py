#!/usr/bin/env python3
"""Read what the kernel recorded when the browser died, instead of guessing.

Every SIGSEGV on macOS leaves an `.ips` report naming the exception type, the
termination reason and the faulting frames. Two independent Chromium builds die
the same way on this host, so the answer is in what they share: the loader, the
libraries interposed into them, and the signature policy applied to them.

This prints the newest crash report for any Chromium-family process, plus the
loader state that can make an unrelated build crash identically.
"""

import json
import os
import pathlib
import plistlib
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
REPORTS = [HOME / "Library" / "Logs" / "DiagnosticReports", pathlib.Path("/Library/Logs/DiagnosticReports")]
FAMILY = ("Chromium", "Google Chrome", "chrome_crashpad", "Chromium Helper")
FRAMES = 14


def newest_reports(limit=3):
    found = []
    for root in REPORTS:
        if not root.is_dir():
            continue
        for entry in root.glob("*.ips"):
            if entry.name.startswith(FAMILY):
                found.append(entry)
    return sorted(found, key=lambda path: path.stat().st_mtime, reverse=True)[:limit]


def summarise(path):
    print(f"== {path.name}")
    text = path.read_text(encoding="utf-8", errors="replace")
    head, _, body = text.partition("\n")
    try:
        report = json.loads(body)
    except json.JSONDecodeError:
        print(f"   unparseable report; first line {head[: len('a' * 120)]}")
        return NONE
    exception = report.get("exception", {})
    print(f"   exception   {exception.get('type')} {exception.get('signal')} subtype {exception.get('subtype')}")
    if report.get("termination"):
        termination = report["termination"]
        print(f"   termination {termination.get('namespace')} code {termination.get('code')} {termination.get('reasons')}")
    if report.get("vmregioninfo"):
        print(f"   region      {report['vmregioninfo'][: len('a' * 150)]}")
    images = report.get("usedImages", [])
    faulting = next((thread for thread in report.get("threads", []) if thread.get("triggered")), NONE)
    if not faulting:
        print("   no triggered thread recorded")
        return NONE
    print(f"   thread      {faulting.get('name') or faulting.get('queue') or '(unnamed)'}")
    for frame in faulting.get("frames", [])[:FRAMES]:
        index = frame.get("imageIndex", ZERO)
        image = images[index] if index < len(images) else {}
        name = image.get("name", "?")
        symbol = frame.get("symbol") or f"+{frame.get('imageOffset')}"
        print(f"     {name:34} {symbol}")
    return NONE


def loader_state():
    print("== loader")
    for key in ("DYLD_INSERT_LIBRARIES", "DYLD_LIBRARY_PATH", "DYLD_FRAMEWORK_PATH", "MallocStackLogging"):
        proc = subprocess.run(["/bin/launchctl", "getenv", key], capture_output=True, text=True, check=False)
        value = proc.stdout.strip()
        print(f"   launchd {key:22} {value or '(unset)'}")
        print(f"   session {key:22} {os.environ.get(key, '(unset)')}")
    for path in (pathlib.Path("/etc/launchd.conf"), pathlib.Path("/etc/paths.d")):
        print(f"   {path} {'present' if path.exists() else 'absent'}")


def extensions():
    print("== system extensions and interposers")
    proc = subprocess.run(["/usr/bin/systemextensionsctl", "list"], capture_output=True, text=True, check=False)
    lines = [line.strip() for line in proc.stdout.splitlines() if line.strip()]
    for line in lines[: len("a" * 12)]:
        print(f"   {line[: len('a' * 150)]}")
    if not lines:
        print("   none reported")
    for root in (pathlib.Path("/Library/Application Support"),):
        for vendor in ("CrowdStrike", "SentinelOne", "Sophos", "Jamf", "Microsoft/Defender", "Cisco"):
            path = root / vendor
            if path.exists():
                print(f"   vendor agent present {path}")


def signatures():
    print("== signatures")
    binaries = []
    playwright = HOME / "Library" / "Caches" / "ms-playwright"
    binaries += sorted(playwright.glob("chromium*/chrome-mac/Chromium.app/Contents/MacOS/Chromium"))
    binaries += sorted((HOME / ".local" / "share" / "weles-chromium").glob("*/Chromium.app/Contents/MacOS/Chromium"))
    for binary in binaries:
        proc = subprocess.run(
            ["/usr/bin/codesign", "--verify", "--deep", "--strict", "--verbose=2", str(binary.parents[2])],
            capture_output=True,
            text=True,
            check=False,
        )
        verdict = (proc.stderr.strip().splitlines() or ["(silent)"])[-1]
        quarantine = subprocess.run(
            ["/usr/bin/xattr", "-p", "com.apple.quarantine", str(binary)],
            capture_output=True,
            text=True,
            check=False,
        )
        print(f"   {binary.parents[2].name}")
        print(f"     verify     {verdict[: len('a' * 130)]}")
        print(f"     quarantine {quarantine.stdout.strip() or '(none)'}")


def main():
    reports = newest_reports()
    if not reports:
        print("no Chromium crash reports on this host")
    for path in reports:
        summarise(path)
    loader_state()
    extensions()
    signatures()
    return NONE


sys.exit(main())
