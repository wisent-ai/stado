#!/usr/bin/env python3
"""Measure what this host can actually do, and print it as one capability document.

For weeks the fleet had no way to say "this job needs a display". The Weles
login jobs drive a headed browser; installed as LaunchDaemons on a machine
nobody is logged into, they landed in launchd's `Background` session, which has
no WindowServer, and Chromium died inside `ScopedCGWindowID` while Playwright
reported only `pwBrowser disconnected`. Every layer above re-diagnosed that as
its own bug because no layer could ask the host what it was capable of.

This script answers that question by measurement, never by assumption:

  display        can a process started by this host's scheduling mechanism own
                 a window right now
  browser-render can the exact Chromium this host deploys for Weles start and
                 produce pixels
  os             what this machine is, so a reader can tell which rules apply

It writes `wisent.host-capabilities.v1` to stdout and nothing else, so the
publisher can pipe it straight into the object API and a human can run it alone
to see the evidence. It changes nothing on the host: no login window is opened,
no launchd domain is created, no security posture is touched.
"""

import base64
import datetime
import json
import os
import pathlib
import platform
import shutil
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

NONE = None
ZERO = len([])
# The registry is read once per run: two probes need it and `stado registry pull`
# is a network call.
TARGET = None
SCHEMA = "wisent.host-capabilities.v1"
HOME = pathlib.Path(os.path.expanduser("~"))
STADO = HOME / ".stado" / "bin" / "stado"
NAMESPACE = "probierz"
# Chromium has to start, resolve a data: URL, rasterise it and exit; a healthy
# headless start does that in a few seconds. The bound is short enough that the
# publisher can run on the health beacon's cadence even when the browser never
# returns -- which, on this fleet's Chromium 147 build, is what happens every
# time -- and long enough that a slow browser is never called broken.
BROWSER_TIMEOUT = len("a" * 20)
# A headed browser has more to do before it can answer: create a window, reach
# the window server, and open its debugging port.
HEADED_TIMEOUT = len("a" * 25)
CDP_TIMEOUT = len("a" * 10)
POLL_INTERVAL = len("ab") / len("aaaa")
REGISTRY_TIMEOUT = len("a" * 30)
# A PNG that decoded a heading is thousands of bytes; a file the browser created
# and never filled is what a crashed compositor leaves behind, so bytes on disk
# is the whole test.
EMPTY_RENDER = ZERO
PROBE_PAGE = "data:text/html,<title>probe</title><h1>probe</h1>"


def run(*args, timeout=REGISTRY_TIMEOUT):
    """One bounded external measurement: exit code and combined output."""
    try:
        proc = subprocess.run(
            args, capture_output=True, text=True, timeout=timeout, check=False
        )
    except subprocess.TimeoutExpired:
        return NONE, f"timed out after {timeout}s"
    except OSError as error:
        return NONE, str(error)
    return proc.returncode, (proc.stdout + proc.stderr).strip()


def registry_document():
    """The registry, read the way a host with no control plane can still read it.

    The capability document is keyed by registry target name, so the measurement
    needs the registry. Asking the object API for it is fine when the API is up,
    and a host whose API is down still has to be able to report that it cannot
    render, so the last-known-good copy on disk is the fallback.
    """
    code, output = run(str(STADO), "registry", "pull")
    if code == ZERO:
        try:
            return json.loads(output)
        except ValueError:
            pass
    for candidate in (
        HOME / ".stado" / "local-storage" / "registry.json",
        HOME / ".stado" / "local-storage" / "ecosystem" / NAMESPACE / "registry.json",
        HOME / ".stado" / "local-backup" / "registry.json",
        HOME / ".stado" / "files" / "registry-next.json",
    ):
        if candidate.is_file():
            try:
                return json.loads(candidate.read_text(encoding="utf-8"))
            except ValueError:
                continue
    return {}


def this_target():
    """The registry entry for this machine, read once and remembered.

    The object key has to be the registry name, because that is the name every
    other layer -- placement, the doctor, the requirement declaration -- uses.
    A host that cannot find itself in the registry says so instead of inventing
    a name that nobody would ever read back.
    """
    global TARGET
    if TARGET is not NONE:
        return TARGET
    node = socket.gethostname().lower()
    short = node.split(".")[ZERO]
    for entry in registry_document().get("targets", []):
        names = [str(name).lower() for name in entry.get("hostnames", [])]
        names.append(str(entry.get("name", "")).lower())
        if any(name == node or name.split(".")[ZERO] == short for name in names if name):
            TARGET = entry
            return TARGET
    raise SystemExit(f"no registry target matches this machine ({node})")


def registry_target_name():
    return this_target().get("name")


def declared_account():
    """The account the fleet schedules work as here, and its uid.

    The measurement must describe the session of the account the fleet uses, not
    of whoever happens to be running the measurement. A LaunchDaemon runs as
    root, root has no Aqua session on any machine in this fleet, and the first
    version of this probe therefore reported `gui/0 absent` -- true, and about a
    user nobody schedules anything as. The registry already names the account in
    the target's ssh coordinate, so that is the one inspected.
    """
    coordinate = str(this_target().get("ssh", ""))
    account = coordinate.split("@")[ZERO].strip() if "@" in coordinate else ""
    if not account:
        return NONE, os.getuid()
    code, output = run("/usr/bin/id", "-u", account)
    if code != ZERO or not output.strip().isdigit():
        return account, os.getuid()
    return account, int(output.strip())


def macos_display():
    """Whether launchd here can put a job in a session that owns a window.

    Two separate measurements, because they answer two different halves and the
    fleet was bitten by conflating them. `launchctl managername` names the
    session THIS process is in -- an ssh channel and a LaunchDaemon both say
    `Background`, and a job in `Background` has no WindowServer at all.
    `launchctl print gui/<uid>` answers whether an Aqua session exists for this
    account at this moment, which is the only thing that decides whether a
    LaunchAgent could be placed somewhere a window can exist. A host is
    display-capable when that Aqua domain is there; the session name is carried
    in the detail so a reader can see which of the two facts they are looking at.
    """
    account, uid = declared_account()
    named = f"gui/{uid}" + (f" ({account}, the account the registry declares)" if account else "")
    _, manager = run("/bin/launchctl", "managername")
    manager = manager.splitlines()[ZERO].strip() if manager else "(no answer)"
    code, _ = run("/bin/launchctl", "print", f"gui/{uid}")
    aqua_domain = code == ZERO
    if aqua_domain:
        detail = (
            f"launchd session {manager}; {named} answers, so an Aqua session exists for "
            f"uid {uid} and a LaunchAgent bootstrapped into it can own a window"
        )
    else:
        detail = (
            f"launchd session {manager}; {named} absent, so launchd can only start this "
            "host's jobs in the Background session, which has no WindowServer"
        )
    return aqua_domain or manager == "Aqua", detail, {}


def reachable_socket(path):
    """Whether something is listening on this unix socket right now.

    The environment of an ssh helper is not the environment of the worker unit,
    so `$DISPLAY` here proves nothing either way: a host can run Xvfb for its
    jobs and hand this channel no variable at all, and a stale variable can name
    a display that died. Connecting is the measurement.
    """
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as probe:
        probe.settimeout(POLL_INTERVAL)
        try:
            probe.connect(str(path))
        except OSError:
            return False
    return True


def linux_display():
    """Whether an X or Wayland display exists that a worker here could reach."""
    x11 = pathlib.Path("/tmp/.X11-unix")
    runtime = pathlib.Path(os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}"))
    live_x = [
        f":{entry.name[len('X'):]}"
        for entry in sorted(x11.glob("X*"))
        if reachable_socket(entry)
    ] if x11.is_dir() else []
    live_wayland = [
        entry.name
        for entry in sorted(runtime.glob("wayland-*"))
        if not entry.name.endswith(".lock") and reachable_socket(entry)
    ] if runtime.is_dir() else []
    declared = os.environ.get("DISPLAY", "")
    wayland = os.environ.get("WAYLAND_DISPLAY", "")
    environment = {}
    if live_x:
        environment["DISPLAY"] = declared if declared in live_x else live_x[ZERO]
    if live_wayland:
        environment["WAYLAND_DISPLAY"] = (
            wayland if wayland in live_wayland else live_wayland[ZERO]
        )
        environment["XDG_RUNTIME_DIR"] = str(runtime)
    detail = (
        f"X displays answering in {x11}: {', '.join(live_x) or 'none'}; "
        f"wayland sockets answering in {runtime}: {', '.join(live_wayland) or 'none'}; "
        f"this channel was handed DISPLAY={declared or '(unset)'} "
        f"WAYLAND_DISPLAY={wayland or '(unset)'}"
    )
    return bool(live_x or live_wayland), detail, environment


def weles_browser():
    """The exact Chromium this host deploys for Weles, newest first.

    Measuring a stock Chrome from /Applications would answer a question nobody
    asked: Weles runs the browser it downloads under `~/.local/share`, and that
    is the binary whose behaviour decides whether a login trajectory can run.
    """
    root = HOME / ".local" / "share" / "weles-chromium"
    patterns = (
        "*/Chromium.app/Contents/MacOS/Chromium",
        "*/chrome-mac/Chromium.app/Contents/MacOS/Chromium",
        "*/chromium/chrome",
        "*/chrome-linux/chrome",
    )
    found = []
    for pattern in patterns:
        found.extend(path for path in root.glob(pattern) if os.access(path, os.X_OK))
    if not found:
        return NONE
    return max(found, key=lambda path: path.stat().st_mtime)


def free_port():
    """A port the browser can have to itself for one measurement."""
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", ZERO))
        return probe.getsockname()[-len("a")]


def websocket(url, request):
    """One CDP request over a websocket, using nothing but the standard library.

    A headed browser has no `--screenshot` switch -- that one is headless-only --
    so the only way to prove a real window painted a frame is to ask the browser
    for the frame through its own debugging protocol. Pulling in a websocket
    dependency for eighty bytes of framing would put a package manager between
    this fleet and its ability to measure itself, so the frame is spoken here:
    the RFC 6455 handshake, one masked client frame, and reassembly of the
    server's reply, which arrives fragmented because a PNG does not fit in one.
    """
    address = url.split("://", len("a"))[-1]
    host, _, path = address.partition("/")
    hostname, _, port = host.partition(":")
    key = base64.b64encode(os.urandom(len("a" * 16))).decode("ascii")
    handshake = (
        f"GET /{path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\n"
        f"Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    )
    with socket.create_connection((hostname, int(port)), timeout=CDP_TIMEOUT) as stream:
        stream.settimeout(CDP_TIMEOUT)
        stream.sendall(handshake.encode("ascii"))
        head = b""
        while b"\r\n\r\n" not in head:
            chunk = stream.recv(len("a" * 4096))
            if not chunk:
                raise OSError("the browser closed the debugging socket during the handshake")
            head += chunk
        if b" 101 " not in head.split(b"\r\n")[ZERO]:
            raise OSError(f"the browser refused the websocket: {head.splitlines()[ZERO]!r}")
        payload = json.dumps(request).encode("utf-8")
        mask = os.urandom(len("mask"))
        header = bytearray([0x81])
        if len(payload) < len("a" * 126):
            header.append(0x80 | len(payload))
        elif len(payload) < len("a" * 65536):
            header.append(0x80 | len("a" * 126))
            header += struct.pack(">H", len(payload))
        else:
            header.append(0x80 | len("a" * 127))
            header += struct.pack(">Q", len(payload))
        header += mask
        stream.sendall(
            bytes(header) + bytes(byte ^ mask[index % len(mask)] for index, byte in enumerate(payload))
        )

        def take(count):
            buffer = b""
            while len(buffer) < count:
                chunk = stream.recv(count - len(buffer))
                if not chunk:
                    raise OSError("the browser closed the debugging socket mid-frame")
                buffer += chunk
            return buffer

        message = b""
        while True:
            first, second = take(len("ab"))
            length = second & 0x7F
            if length == len("a" * 126):
                length = struct.unpack(">H", take(len("ab")))[ZERO]
            elif length == len("a" * 127):
                length = struct.unpack(">Q", take(len("abcdefgh")))[ZERO]
            message += take(length)
            if first & 0x80:
                break
        return json.loads(message)


def headed_render(binary, scratch, environment):
    """Open a real window, ask the window for its pixels, and count them.

    This is the mode the login trajectories actually use: Kimi and Claude drive a
    headed browser on purpose. Measuring headless and calling the answer
    `browser-render` would refuse the one host that can run those jobs on
    evidence about a different mode entirely.

    The browser is killed as soon as the frame is in hand. A measurement is not
    a session, and leaving a window open on somebody's desk is not a measurement.
    """
    port = free_port()
    profile = scratch / "headed-profile"
    log = scratch / "headed.log"
    command = [
        str(binary),
        "--no-first-run",
        "--no-default-browser-check",
        "--no-sandbox",
        f"--user-data-dir={profile}",
        f"--remote-debugging-port={port}",
        "--window-size=400,300",
        # This runs once a minute on a machine somebody is using. A real window
        # is the measurement and cannot be given up, but it does not have to be
        # where anybody can see it: parked fully outside every display it still
        # paints frames for Page.captureScreenshot, and a launch watched while a
        # fullscreen game held the front never took focus (2026-08-21).
        "--window-position=-32000,-32000",
        PROBE_PAGE,
    ]
    # The worker's display is not this channel's display: on Linux the unit that
    # runs browser journeys is handed an Xvfb or Wayland socket that an ssh
    # helper never sees, so the browser is started with the display the previous
    # measurement actually connected to.
    with log.open("wb") as sink:
        browser = subprocess.Popen(
            command,
            stdout=sink,
            stderr=sink,
            stdin=subprocess.DEVNULL,
            start_new_session=True,
            env=dict(os.environ, **environment),
        )
    try:
        deadline = time.time() + HEADED_TIMEOUT
        target = NONE
        while time.time() < deadline and target is NONE:
            if browser.poll() is not NONE:
                noise = [
                    line
                    for line in log.read_text("utf-8", "replace").splitlines()
                    if line.strip()
                ]
                return (
                    False,
                    f"{binary} headed exited {browser.returncode} before its window answered; "
                    f"last output {noise[-len('a')] if noise else '(silent)'}",
                )
            try:
                with urllib.request.urlopen(
                    f"http://127.0.0.1:{port}/json/list", timeout=CDP_TIMEOUT
                ) as answer:
                    targets = json.loads(answer.read().decode("utf-8"))
                target = next(
                    (entry for entry in targets if entry.get("type") == "page"), NONE
                )
            except (urllib.error.URLError, OSError, ValueError):
                time.sleep(POLL_INTERVAL)
        if target is NONE:
            return (
                False,
                f"{binary} headed never published a page target on its debugging port "
                f"within {HEADED_TIMEOUT}s",
            )
        answer = websocket(
            target["webSocketDebuggerUrl"],
            {"id": len("a"), "method": "Page.captureScreenshot", "params": {"format": "png"}},
        )
        data = answer.get("result", {}).get("data", "")
        frame = base64.b64decode(data) if data else b""
        if not frame:
            return (
                False,
                f"{binary} headed opened a window titled {target.get('title', '')!r} but "
                f"Page.captureScreenshot returned no frame: {json.dumps(answer)[: len('a' * 160)]}",
            )
        # The target list was read while the page was still loading, so its title
        # was empty then. Reading it again after the frame is in hand names the
        # document that was actually photographed.
        title = target.get("title", "")
        try:
            with urllib.request.urlopen(
                f"http://127.0.0.1:{port}/json/list", timeout=CDP_TIMEOUT
            ) as answer:
                for entry in json.loads(answer.read().decode("utf-8")):
                    if entry.get("id") == target.get("id"):
                        title = entry.get("title", title)
        except (urllib.error.URLError, OSError, ValueError):
            pass
        return (
            True,
            f"{binary} headed captured {len(frame)} bytes of PNG through the debugging "
            f"protocol from a window titled {title!r}",
        )
    finally:
        # The window goes away with the process group: a headed Chromium spawns
        # helpers that outlive a plain kill, and a measurement that leaves a
        # browser running has changed the host it was meant to observe.
        try:
            os.killpg(os.getpgid(browser.pid), signal.SIGKILL)
        except (ProcessLookupError, PermissionError):
            pass
        browser.wait()


def headless_render(binary, scratch):
    """Start that browser headless and count the bytes it puts on disk.

    A screenshot is the whole measurement: it proves the browser started, parsed
    a document, reached the compositor and wrote pixels. Anything short of that
    -- a crash, a missing library, a sandbox refusal -- leaves a zero-byte file
    or no file at all, and the byte count is reported either way so the reader
    sees the evidence rather than a verdict.

    Chromium is not one process, and that shapes how it must be run here. Its
    helpers inherit whatever stdio they are given, so a pipe stays open after
    the browser itself has died and a reader waiting for end-of-file waits for
    the helpers instead -- a crash that takes a second then costs the full
    timeout and reports "timed out" for a browser that in fact died instantly.
    The output goes to files, the browser gets its own process group, and a
    timeout kills the group rather than the one process the fleet happened to
    have a handle on.
    """
    shot = scratch / "headless.png"
    profile = scratch / "headless-profile"
    log = scratch / "headless.log"
    command = [
        str(binary),
        "--headless=new",
        "--no-sandbox",
        "--disable-gpu",
        # A first-run prompt or a default-browser check is a dialog, and a dialog
        # on a host with no window server is a hang rather than an answer. Weles
        # starts its own browser with both suppressed.
        "--no-first-run",
        "--no-default-browser-check",
        f"--user-data-dir={profile}",
        f"--screenshot={shot}",
        PROBE_PAGE,
    ]
    with log.open("wb") as sink:
        browser = subprocess.Popen(
            command, stdout=sink, stderr=sink, stdin=subprocess.DEVNULL, start_new_session=True
        )
        try:
            code = browser.wait(timeout=BROWSER_TIMEOUT)
            killed = False
        except subprocess.TimeoutExpired:
            os.killpg(os.getpgid(browser.pid), signal.SIGKILL)
            code = browser.wait()
            killed = True
    written = shot.stat().st_size if shot.is_file() else ZERO
    # The exit code alone is not the answer: a browser can exit 0 after writing
    # nothing, and can write a valid screenshot while complaining on stderr. The
    # bytes decide, the exit code is context. A negative code is a signal, and
    # -11 is the SIGSEGV a Chromium without a window server dies of.
    rendered = written > EMPTY_RENDER
    # "Killed at the bound" and "died on its own" are different faults and the
    # reader has to be able to tell them apart: a hang means no headless journey
    # on this host would ever return either, while a signal means the browser
    # reached something it could not have.
    outcome = (
        f"never exited and was killed after {BROWSER_TIMEOUT}s (a hang in the browser, "
        "not a refusal by this host)"
        if killed
        else f"exited {code}"
    )
    detail = f"{binary} --headless=new wrote {written} bytes to a PNG and {outcome}"
    if not rendered:
        noise = [line for line in log.read_text("utf-8", "replace").splitlines() if line.strip()]
        detail = f"{detail}; last output {noise[-len('a')] if noise else '(silent)'}"[
            : len("a" * 400)
        ]
    return rendered, detail


def render_capabilities(display, environment):
    """Both rendering measurements, in one scratch directory.

    `browser-render` answers the question the login trajectories ask, so it is
    measured in the mode those trajectories use: headed wherever a display
    exists, headless only where one does not, and the detail says which. The
    headless result is kept as its own capability whatever happens, because it
    is how this fleet discovers that its Chromium build hangs in headless mode,
    and a fix to one measurement must not erase the other.
    """
    binary = weles_browser()
    if binary is NONE:
        absent = "no Weles Chromium is deployed under ~/.local/share/weles-chromium"
        return (False, absent), (False, absent)
    # The Stado host agent runs helpers under a secret-safe umask that strips the
    # execute bit, so every directory created here would land unsearchable and
    # the browser could not enter the profile it had just made. That is a
    # property of the channel, not of the host, and measuring it as a rendering
    # failure would be a lie.
    os.umask(0o022)
    scratch = pathlib.Path(tempfile.mkdtemp(prefix="host-capabilities-"))
    try:
        headless = headless_render(binary, scratch)
        if not display:
            return (
                headless[ZERO],
                f"measured headless because this host has no display: {headless[-len('a')]}",
            ), headless
        headed = headed_render(binary, scratch, environment)
        return (
            headed[ZERO],
            f"measured headed, the mode the login trajectories use: {headed[-len('a')]}",
        ), headless
    finally:
        shutil.rmtree(scratch, ignore_errors=True)


def operating_system():
    """What this machine is. Always true: every host has an OS, and the point of
    the field is to carry the version a reader needs to judge the other two."""
    if platform.system() == "Darwin":
        _, product = run("/usr/bin/sw_vers", "-productVersion")
        _, name = run("/usr/bin/sw_vers", "-productName")
        return True, f"{name.strip() or 'macOS'} {product.strip()} {platform.machine()}"
    release = ""
    os_release = pathlib.Path("/etc/os-release")
    if os_release.is_file():
        for line in os_release.read_text(encoding="utf-8").splitlines():
            if line.startswith("PRETTY_NAME="):
                release = line.split("=", len("a"))[-1].strip().strip('"')
    _, kernel = run("/bin/uname", "-sr")
    return True, f"{release or platform.system()} kernel {kernel.strip()} {platform.machine()}"


def main():
    display = macos_display() if platform.system() == "Darwin" else linux_display()
    rendered, headless = render_capabilities(display[ZERO], display[-len("a")])
    system = operating_system()
    document = {
        "schema": SCHEMA,
        "host": registry_target_name(),
        "measured_at": datetime.datetime.now(datetime.timezone.utc)
        .replace(microsecond=ZERO)
        .isoformat()
        .replace("+00:00", "Z"),
        "capabilities": {
            "display": {"value": display[ZERO], "detail": display[len("a")]},
            "browser-render": {"value": rendered[ZERO], "detail": rendered[-len("a")]},
            "browser-render-headless": {
                "value": headless[ZERO],
                "detail": headless[-len("a")],
            },
            "os": {"value": system[ZERO], "detail": system[-len("a")]},
        },
    }
    print(json.dumps(document, indent=len("ba"), sort_keys=False))
    return NONE


sys.exit(main())
