#!/usr/bin/env python3
"""Publish this host's capabilities on a schedule, in the domain its beacon uses.

A capability object stops being an answer the moment it goes stale: placement
and `registry doctor` both judge a measurement against the host's beacon
liveness window, so a host measured once by hand is unusable again three minutes
later. The measurement therefore has to run the way every other per-host
publication on this fleet runs -- on a timer, owned by the host, with a log.

Which lifecycle mechanism is not a choice this script makes. It finds the health
beacon the host already runs and installs the capability publisher beside it:
the same launchd domain on macOS, the same systemd manager on Linux, the same
cadence. That rule is the point of the pass this belongs to. The Weles login
jobs were installed as LaunchDaemons on a machine with no console session,
landed in the `Background` session and died in the window server every time;
installing the measurer that would have caught it into a domain the host does
not have would repeat the fault one layer down.

Idempotent: the unit is rewritten only when its text differs and reloaded only
when it was rewritten or is not loaded. Prints what it found, what it wrote, and
what launchd or systemd says afterwards.
"""

import hashlib
import json
import os
import pathlib
import platform
import shutil
import subprocess
import sys
import time

NONE = None
ZERO = len([])
HOME = pathlib.Path(os.path.expanduser("~"))
FILES = HOME / ".stado" / "files"
PUBLISHER = HOME / ".stado" / "bin" / "publish-host-capabilities"
LABEL = "com.wisent.host-capabilities"
BEACON = "com.wisent.host-health-beacon"
DAEMONS = pathlib.Path("/Library/LaunchDaemons")
AGENTS = HOME / "Library" / "LaunchAgents"
SYSTEMD = pathlib.Path("/etc/systemd/system")
# The health beacon publishes every sixty seconds and both readers of these
# objects apply the beacon's own liveness window to them, so any slower cadence
# would leave the host looking unmeasured for part of every cycle. The
# measurement is bounded well inside this interval, browser hangs included.
INTERVAL = len("a" * 60)
# The forced first run measures a browser, so it takes tens of seconds; the
# watch has to outlast it or the installer would report an empty log for a unit
# that was working.
WATCH = len("a" * 120)
POLL = len("aaa")
# An interpreter that has not printed a word in this long will not print one in
# a scheduled unit either, and waiting longer only delays the fallback.
AUDITION = len("a" * 20)


def run(*args, timeout=len("a" * 120)):
    try:
        proc = subprocess.run(
            args, capture_output=True, text=True, check=False, timeout=timeout
        )
    except subprocess.TimeoutExpired:
        return NONE, f"timed out after {timeout}s"
    return proc.returncode, (proc.stdout + proc.stderr).strip()


def privileged(*args):
    """Run as root, without asking for a password this channel cannot answer."""
    if os.getuid() == ZERO:
        return run(*args)
    return run("/usr/bin/sudo", "-n", *args)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()[: len("a" * 12)] if path.is_file() else "-"


def delivered(name):
    path = FILES / name
    if not path.is_file():
        raise SystemExit(
            f"{path} is absent; deliver it with `stado host install-file <host> "
            f"deploy/{name} {name}`"
        )
    return path.read_text(encoding="utf-8")


def write_unit(path, body, as_root):
    """Install one unit file, and say whether it changed anything."""
    if path.is_file() and path.read_text(encoding="utf-8") == body:
        print(f"settled     {path} already matches  sha256 {digest(path)}")
        return False
    staging = FILES / f"{path.name}.staged"
    staging.write_text(body, encoding="utf-8")
    if as_root:
        code, output = privileged("/bin/cp", str(staging), str(path))
        if code != ZERO:
            raise SystemExit(f"could not write {path}: {output}")
        privileged("/bin/chmod", "644", str(path))
    else:
        path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(staging, path)
        path.chmod(0o644)
    staging.unlink()
    print(f"wrote       {path}  sha256 {digest(path)}")
    return True


def beacon_unit():
    """Where this host's health beacon lives, and therefore where this one goes."""
    if platform.system() == "Darwin":
        for domain, folder, as_root in (
            ("system", DAEMONS, True),
            (f"gui/{os.getuid()}", AGENTS, False),
        ):
            candidate = folder / f"{BEACON}.plist"
            if candidate.is_file():
                return domain, folder, as_root, candidate
        raise SystemExit(
            f"no {BEACON} unit in {DAEMONS} or {AGENTS}; this host runs no beacon to sit beside"
        )
    for name in ("host-health-beacon.timer", "host-health-beacon.service"):
        candidate = SYSTEMD / name
        if candidate.is_file():
            return "system", SYSTEMD, True, candidate
    # This fleet's Linux member runs no beacon unit of its own: it is measured
    # locally and handed in by an operator. That is a fact about the beacon, not
    # an argument for leaving capabilities unscheduled, and now that the host can
    # reach the store directly its own init is the mechanism it does have. The
    # absence is reported rather than papered over, because a reader comparing
    # the two publications should know they are not on the same footing.
    print(f"beacon      none in {SYSTEMD}; this host hands its beacon in, so systemd is the cadence")
    return "system", SYSTEMD, True, NONE


def log_path(plist):
    """The log directory the host's own beacon writes to, reused verbatim."""
    code, printed = run("/usr/bin/plutil", "-convert", "json", "-o", "-", str(plist))
    if code == ZERO:
        out = json.loads(printed).get("StandardOutPath")
        if out:
            return pathlib.Path(out).parent / f"{LABEL}.log"
    return HOME / ".stado" / "logs" / f"{LABEL}.log"


def beacon_settings(plist):
    """The beacon unit as launchd sees it, so this unit can copy its account."""
    code, printed = run("/usr/bin/plutil", "-convert", "json", "-o", "-", str(plist))
    return json.loads(printed) if code == ZERO else {}


def watch(log, launch=NONE, domain=NONE):
    """Wait for the forced run to finish, then print what it wrote.

    "The unit is loaded" is not the claim worth making; "the unit published" is.
    The measurement takes tens of seconds because it waits on a browser, so the
    log is followed until it stops growing rather than sampled once.

    A unit that logged nothing at all has usually failed before its program ever
    ran -- a spawn error, a missing interpreter, an unwritable path -- and
    launchd keeps that reason in its own record, so the record is what gets
    printed instead of an unexplained silence.
    """
    settled = ZERO
    deadline = time.time() + WATCH
    while time.time() < deadline:
        size = log.stat().st_size if log.is_file() else ZERO
        if size and size == settled:
            break
        settled = size
        time.sleep(POLL)
    lines = log.read_text("utf-8", "replace").splitlines() if log.is_file() else []
    for line in lines[-len("a" * 8):]:
        print(f"ran         {line[: len('a' * 150)]}")
    if lines or launch is NONE:
        return
    print(f"ran         {log} is still empty {WATCH}s after the unit was kicked")
    code, printed = launch("/bin/launchctl", "print", f"{domain}/{LABEL}")
    pid = NONE
    for line in printed.splitlines():
        stripped = line.strip()
        if stripped.startswith("pid = "):
            pid = stripped.split("=", len("a"))[-1].strip()
        if stripped.startswith(
            ("state =", "runs =", "last exit code =", "spawn type", "program =", "path =", "error")
        ):
            print(f"record      {stripped[: len('a' * 150)]}")
    # A job still running with an empty log is blocked, and a block has an
    # address. Sampling it names the call it is sitting in, which is the only
    # thing that distinguishes "slow measurement" from "wedged forever".
    if pid:
        code, sampled = run("/usr/bin/sample", pid, str(POLL))
        # The deepest frames are the answer, and guessing which ones are
        # interesting is how the last three probes printed nothing useful; the
        # head of the main thread's stack is printed as it comes.
        main_thread = sampled.partition("Binary Images")[ZERO]
        body = main_thread.partition("Call graph:")[-len("a")] or main_thread
        for line in [line for line in body.splitlines() if line.strip()][: len("a" * 22)]:
            print(f"blocked     {line.rstrip()[: len('a' * 150)]}")


def interpreter():
    """A python that actually starts inside a scheduled unit.

    `sys.executable` here is whichever interpreter the operator channel happened
    to use, and the two Macs in this fleet disagree about which ones survive a
    scheduled start. The operator laptop's miniforge build never returns from
    `init_import_site` under launchd -- the unit sat at `state = running`
    forever, wrote an empty log and published nothing, while the same script
    over ssh finished in ninety seconds. The always-on host's `/usr/bin/python3`
    is an Xcode shim that exits `EX_CONFIG` in a session with no developer
    context. Neither failure is visible from a normal shell.

    So the candidates are auditioned rather than assumed, in an environment
    stripped the way launchd strips one, with a bound short enough that the
    laptop's hang disqualifies its interpreter instead of hanging this script.
    """
    audition = "import json, ssl, urllib.request; print('ready')"
    for candidate in ("/usr/bin/python3", "/opt/homebrew/bin/python3", sys.executable):
        if not pathlib.Path(candidate).is_file():
            continue
        code, output = run(
            "/usr/bin/env",
            "-i",
            f"HOME={HOME}",
            "PATH=/usr/bin:/bin",
            candidate,
            "-c",
            audition,
            timeout=AUDITION,
        )
        print(f"audition    {candidate}  exit {code}  {output.splitlines()[-len('a'):] or ['(silent)']}")
        if code == ZERO and "ready" in output:
            return candidate
    raise SystemExit(
        "no interpreter on this host starts cleanly enough to run a scheduled unit"
    )


def install_macos(domain, folder, as_root, beacon, python):
    log = log_path(beacon)
    body = (
        delivered(f"{LABEL}.plist.tmpl")
        .replace("{LABEL}", LABEL)
        .replace("{PYTHON}", python)
        .replace("{SCRIPT_PATH}", str(PUBLISHER))
        .replace("{INTERVAL_SECONDS}", str(INTERVAL))
        .replace("{LOG_PATH}", str(log))
        .replace("{HOME}", str(HOME))
    )
    plist = folder / f"{LABEL}.plist"
    # The unit deliberately carries no UserName: launchd answered EX_CONFIG for
    # every start once one was added, and the account this measurement must
    # describe is the one the registry declares for the target, which the
    # measurement resolves for itself.
    print(f"account     {beacon_settings(beacon).get('UserName') or 'the domain owner'} runs the beacon")
    print(f"beacon      {domain}  {beacon}")
    print(f"log         {log}")
    log.parent.mkdir(parents=True, exist_ok=True)
    changed = write_unit(plist, body, as_root)

    # The system domain belongs to root and the user's Aqua domain belongs to the
    # user; asking for root in the second case gets a password prompt this
    # channel cannot answer, and would install the job as the wrong owner if it
    # could.
    launch = privileged if as_root else run
    code, _ = launch("/bin/launchctl", "print", f"{domain}/{LABEL}")
    loaded = code == ZERO
    if changed or not loaded:
        if loaded:
            launch("/bin/launchctl", "bootout", f"{domain}/{LABEL}")
        code, output = launch("/bin/launchctl", "bootstrap", domain, str(plist))
        print(f"bootstrap   exit {code}  {output or '(silent)'}")
    else:
        print("bootstrap   (already loaded and unchanged)")
    # A schedule that has not fired yet has proven nothing, so the first
    # publication is forced here and launchd's own record of it is the evidence.
    code, output = launch("/bin/launchctl", "kickstart", "-k", f"{domain}/{LABEL}")
    print(f"kickstart   exit {code}  {output or '(silent)'}")
    code, printed = launch("/bin/launchctl", "print", f"{domain}/{LABEL}")
    for line in printed.splitlines():
        if line.strip().startswith(("state =", "runs =", "last exit code =")):
            print(f"launchd     {line.strip()}")
    watch(log, launch, domain)
    return NONE


def install_linux(python):
    user = HOME.name
    service = (
        delivered("host-capabilities.service")
        .replace("{USER}", user)
        .replace("{GROUP}", user)
        .replace("{HOME}", str(HOME))
        .replace("{PYTHON}", python)
    )
    timer = delivered("host-capabilities.timer").replace("{INTERVAL_SECONDS}", str(INTERVAL))
    changed = write_unit(SYSTEMD / "host-capabilities.service", service, True)
    changed = write_unit(SYSTEMD / "host-capabilities.timer", timer, True) or changed
    if changed:
        code, output = privileged("/usr/bin/systemctl", "daemon-reload")
        print(f"reload      exit {code}  {output or '(silent)'}")
    code, output = privileged("/usr/bin/systemctl", "enable", "--now", "host-capabilities.timer")
    print(f"enable      exit {code}  {output or '(silent)'}")
    code, output = privileged("/usr/bin/systemctl", "start", "host-capabilities.service")
    print(f"start       exit {code}  {output or '(silent)'}")
    for line in run("/usr/bin/systemctl", "list-timers", "--no-pager", "host-capabilities.timer")[
        -len("a")
    ].splitlines()[: len("aaa")]:
        print(f"systemd     {line}")
    code, output = run(
        "/usr/bin/systemctl", "show", "host-capabilities.service", "--property=Result",
        "--property=ExecMainStatus",
    )
    print(f"service     {' '.join(output.split())}")
    return NONE


def main():
    if not PUBLISHER.is_file():
        raise SystemExit(
            f"{PUBLISHER} is absent; install it with `stado host install-helper <host> "
            "scripts/publish-host-capabilities.py publish-host-capabilities`"
        )
    print(f"publisher   {PUBLISHER}  sha256 {digest(PUBLISHER)}")
    print(f"interval    {INTERVAL}s, the cadence of this host's health beacon")
    python = interpreter()
    print(f"interpreter {python}")
    domain, folder, as_root, beacon = beacon_unit()
    if platform.system() == "Darwin":
        return install_macos(domain, folder, as_root, beacon, python)
    return install_linux(python)


sys.exit(main())
