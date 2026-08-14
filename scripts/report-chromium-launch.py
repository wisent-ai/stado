#!/usr/bin/env python3
"""Say whether Weles's Chromium can start at all on this host.

A login that reports `pwBrowser disconnected` immediately after the context is
created is not a page problem: the browser died on launch, and Playwright throws
the failure away. Starting the same binary directly, with its own stderr kept,
turns that into a message.

Read-only: it renders `about:blank` and exits.
"""

import os
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
ROOT = HOME / ".local" / "share" / "weles-chromium"
TIMEOUT = float(len("s" * 60))


def newest_binary():
    builds = sorted(ROOT.glob("*/Chromium.app/Contents/MacOS/Chromium"))
    return builds[-len(["newest"])] if builds else NONE


def main():
    binary = newest_binary()
    print(f"binary     {binary or '(none under ' + str(ROOT) + ')'}")
    if not binary:
        return len("x")
    print(f"  file     {'executable' if os.access(binary, os.X_OK) else 'not executable'}")
    proc = subprocess.run(
        [str(binary), "--version"], capture_output=True, text=True, check=False, timeout=TIMEOUT
    )
    print(f"  version  {(proc.stdout + proc.stderr).strip()[: len('a' * 120)]}")
    profile = HOME / ".local" / "state" / "weles" / "chromium-launch-probe"
    # A launch that hangs or dies says more than a version string, but only if
    # its output is kept, and the mode matters: the trajectories run headed, so
    # a headless-only crash would be a probe artefact rather than the fault.
    for mode in ("--headless=new", "--headless=old", ""):
        command = [str(binary)]
        if mode:
            command.append(mode)
        command += [
            f"--user-data-dir={profile}-{mode.strip('-') or 'headed'}",
            "--no-first-run",
            "--no-default-browser-check",
            "--dump-dom",
            "about:blank",
        ]
        try:
            proc = subprocess.run(
                command, capture_output=True, text=True, check=False, timeout=float(len("s" * 25))
            )
            code, out, err = proc.returncode, proc.stdout, proc.stderr
        except subprocess.TimeoutExpired as expired:
            code = "timed out"
            out = expired.stdout if isinstance(expired.stdout, str) else (expired.stdout or b"").decode("utf-8", "replace")
            err = expired.stderr if isinstance(expired.stderr, str) else (expired.stderr or b"").decode("utf-8", "replace")
        verdict = "crashed (SIGSEGV)" if code == -len("aaaaaaaaaaa") else str(code)
        rendered = "<html" in (out or "")
        print(f"  {mode or 'headed':<16} exit {verdict}  rendered {rendered}")
        # The first frames name the fault; the last two are crashpad tidying up,
        # which is what this printed before and it said nothing.
        head = [line for line in (err or "").splitlines() if line.strip()][: len("llllll")]
        for line in head:
            print(f"    {line[: len('a' * 170)]}")

    # A browser needs the user's Aqua session: from an SSH context the Mach
    # bootstrap is the background one, notification centre and Keychain are
    # unavailable, and the process dies before it paints. `launchctl asuser`
    # places the same command in that session, which is the difference between
    # a broken build and a wrong context.
    session = subprocess.run(
        [
            "/bin/launchctl",
            "asuser",
            str(os.getuid()),
            str(binary),
            "--headless=new",
            f"--user-data-dir={profile}-asuser",
            "--no-first-run",
            "--dump-dom",
            "about:blank",
        ],
        capture_output=True,
        text=True,
        check=False,
        timeout=float(len("s" * 40)),
    )
    print(
        f"  asuser           exit {session.returncode}  "
        f"rendered {'<html' in session.stdout}"
    )
    for line in (session.stderr or "").splitlines()[: len("lll")]:
        if line.strip():
            print(f"    {line[: len('a' * 170)]}")

    # `--dump-dom` is a debugging switch this patched build may not carry. A
    # screenshot exercises the same startup path without it, so the two together
    # separate "the browser is broken" from "the probe asked for the wrong
    # thing".
    shot = pathlib.Path("/tmp/chromium-launch-probe.png")
    plain = subprocess.run(
        [
            str(binary),
            "--headless=new",
            f"--user-data-dir={profile}-shot",
            "--no-first-run",
            f"--screenshot={shot}",
            "about:blank",
        ],
        capture_output=True,
        text=True,
        check=False,
        timeout=float(len("s" * 40)),
    )
    print(
        f"  screenshot       exit {plain.returncode}  "
        f"wrote {shot.is_file() and shot.stat().st_size or 0} bytes"
    )

    # The trajectories launch through `caffeinate -dimsu`, which keeps the
    # machine and its display awake for the run. A browser that renders there
    # and nowhere else makes the wrapper part of the requirement rather than a
    # nicety.
    awake = subprocess.run(
        [
            "/usr/bin/caffeinate",
            "-dimsu",
            str(binary),
            "--headless=new",
            f"--user-data-dir={profile}-awake",
            "--no-first-run",
            f"--screenshot={shot}-awake.png",
            "about:blank",
        ],
        capture_output=True,
        text=True,
        check=False,
        timeout=float(len("s" * 40)),
    )
    awake_shot = pathlib.Path(f"{shot}-awake.png")
    print(
        f"  caffeinate       exit {awake.returncode}  "
        f"wrote {awake_shot.stat().st_size if awake_shot.is_file() else 0} bytes"
    )

    # "Trying to load the allocator multiple times" is what Chromium says when
    # something is injected into it. Anything inherited from the deployment
    # environment would do that to every launch here, so name the variables and
    # try once with a scrubbed environment.
    injected = {
        name: value[: len("a" * 60)]
        for name, value in os.environ.items()
        if name.startswith("DYLD_") or name.startswith("LD_")
    }
    print(f"  injected env     {injected or '(none)'}")
    scrubbed = subprocess.run(
        [
            str(binary),
            "--headless=new",
            f"--user-data-dir={profile}-scrubbed",
            "--no-first-run",
            f"--screenshot={shot}-scrubbed.png",
            "about:blank",
        ],
        capture_output=True,
        text=True,
        check=False,
        timeout=float(len("s" * 40)),
        env={"HOME": str(HOME), "PATH": "/usr/bin:/bin", "USER": os.environ.get("USER", "")},
    )
    written = pathlib.Path(f"{shot}-scrubbed.png")
    print(
        f"  scrubbed env     exit {scrubbed.returncode}  "
        f"wrote {written.stat().st_size if written.is_file() else 0} bytes"
    )
    return NONE


sys.exit(main())
