#!/usr/bin/env python3
"""Install each reauth job where the work can actually run, and say why there.

This replaces `install-reauth-daemons.py`, which put all three jobs into
`/Library/LaunchDaemons` for one reason: the host had no console session, so
`launchctl` refused the login domain and nothing ticked. That made the schedule
work and the work impossible. A LaunchDaemon runs in the `Background` session,
which has no WindowServer, and two of these three jobs log in through a real
browser window. In that session Chromium starts, reaches the network and renders
pages -- so every probe passes -- and then dies the moment it creates a window,
in `ScopedCGWindowID::~ScopedCGWindowID`, which Playwright reports only as
`pwBrowser disconnected`.

The first repair made the *domain* follow from the work. It still left the
*host* to habit: whatever machine happened to run the installer. That is the
same bug one level up, so both now follow from declarations:

  - what the job needs is declared once, by the trajectory that does the work,
    in `weles/scripts/trajectories/requirements.json`;
  - which host can do it is measured and published per host, and matched by
    `place-by-capability.py` against those requirements;
  - a job that needs a display becomes an Aqua LaunchAgent in `gui/<uid>` on the
    host placement named -- installed here when that is this host, and installed
    through Stado on that host when it is not;
  - a job that only signs and posts stays a LaunchDaemon in `system` on the host
    that owns the work, which is the host running this installer.

When no host qualifies, nothing is installed anywhere, the unit is staged, and
the placement refusal is printed candidate by candidate. Silently downgrading a
browser job to a daemon -- or placing it on the always-on host because it is the
one that is always on -- is what produced a year of green schedules and dead
logins.
"""

import argparse
import importlib.machinery
import importlib.util
import json
import os
import pathlib
import plistlib
import platform
import re
import subprocess
import sys
import time

NONE = None
ZERO = len([])
FIRST = len(["first"])
HOME = pathlib.Path(os.path.expanduser("~"))
AGENTS = HOME / "Library" / "LaunchAgents"
DAEMONS = pathlib.Path("/Library/LaunchDaemons")
LOGS = HOME / ".stado" / "logs"
# Where `stado host install-file` lands a unit delivered from another host, and
# where this installer stages its own copies before elevating.
DELIVERED = HOME / ".stado" / "files"
UID = os.getuid()
OWNER_ONLY = 0o600
STADO = pathlib.Path(os.environ.get("STADO_BIN") or HOME / ".stado" / "bin" / "stado")
HELPER_NAME = "install-reauth-jobs"
# A measurement this installer accepts. It runs rarely and by hand, so a window
# wider than the publisher's cadence is fine; anything older than this is a
# claim about a login session that may have ended hours ago.
MAX_STALE_SECONDS = float(len("s" * 900))

# The unit says which trajectory it runs; these are only the last resort for a
# unit whose program cannot be read on this host. A label is not a program --
# `com.wisent.claude-reauth` runs `trajectories/claude/reauth.mjs`, whose
# failure path spawns a headed browser login, and nothing in the name says so.
TRAJECTORY_OF = {
    "com.wisent.codex-reauth": "codex/reauth",
    "com.wisent.claude-reauth": "claude/reauth",
    "com.wisent.kimi-reauth": "kimi/reauth",
}
TRAJECTORY_IN_PATH = re.compile(r"scripts/trajectories/([a-z0-9_]+/[a-z0-9_]+)\.mjs")
# The table this script used to carry alone, kept as the fallback for hosts that
# do not have the Weles tree yet. Once `requirements.json` is present it wins:
# it lives next to the trajectories, so it changes when the work changes, while
# this copy can only be remembered.
FALLBACK_REQUIREMENTS = {
    "codex/reauth": [],
    "claude/reauth": ["display", "browser-render"],
    "kimi/reauth": ["display", "browser-render"],
    "claude/login": ["display", "browser-render"],
    "kimi/login": ["display", "browser-render"],
    "codex/login": ["display", "browser-render"],
}
REQUIREMENTS_CANDIDATES = (
    pathlib.Path(os.environ.get("WELES_TRAJECTORY_REQUIREMENTS", "")),
    HOME / "weles" / "scripts" / "trajectories" / "requirements.json",
    HOME / "Documents" / "CodingProjects" / "Wisent" / "weles" / "scripts" / "trajectories"
    / "requirements.json",
    HOME / ".stado" / "files" / "trajectory-requirements.json",
)
REQUIREMENTS_SCHEMA = "wisent.trajectory-requirements.v1"
# The matcher is one file so operator and installer get the same answer; a
# second copy of the matching is a second answer waiting to disagree.
PLACEMENT_CANDIDATES = (
    pathlib.Path(__file__).resolve().parent / "place-by-capability.py",
    HOME / ".stado" / "bin" / "place-by-capability",
    HOME / ".stado" / "bin" / "place-by-capability.py",
)


def run(*args):
    proc = subprocess.run(args, capture_output=True, text=True, check=False)
    return proc.stdout + proc.stderr


def state(domain, label):
    text = run("/usr/bin/sudo", "-n", "/bin/launchctl", "print", f"{domain}/{label}") if domain == "system" \
        else run("/bin/launchctl", "print", f"{domain}/{label}")
    if not text.strip().startswith(f"{domain}/{label}"):
        return NONE
    found = re.search(r"^\s*state = (.+)$", text, re.MULTILINE)
    return found.group(FIRST).strip() if found else "loaded"


def graphical_domain_exists():
    text = run("/bin/launchctl", "print", f"gui/{UID}")
    return text.strip().startswith(f"gui/{UID}")


def self_target():
    """This machine's registry target name, which is what placement answers in."""
    if not STADO.is_file():
        return NONE
    printed = run(str(STADO), "registry", "self").strip().splitlines()
    if not printed:
        return NONE
    return printed[ZERO].split("\t")[ZERO].strip() or NONE


def requirements_table():
    """Return (source, {trajectory: [capability]}) from the declaration if present."""
    for path in REQUIREMENTS_CANDIDATES:
        if not str(path) or not path.is_file():
            continue
        try:
            document = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, ValueError) as problem:
            return f"{path} (unreadable: {problem}; using the built-in table)", FALLBACK_REQUIREMENTS
        schema = document.get("schema")
        if schema != REQUIREMENTS_SCHEMA:
            return (
                f"{path} (declares schema {schema!r}, not {REQUIREMENTS_SCHEMA}; "
                "using the built-in table)",
                FALLBACK_REQUIREMENTS,
            )
        declared = document.get("trajectories")
        if not isinstance(declared, dict):
            return f"{path} (declares no trajectories; using the built-in table)", FALLBACK_REQUIREMENTS
        return str(path), declared
    return "built-in table (weles requirements.json is not on this host)", FALLBACK_REQUIREMENTS


def placement_module():
    """Import the matcher rather than shelling out, so refusals arrive as data.

    The loader is named explicitly because a Stado helper lands under a bare
    basename with no `.py`, and `spec_from_file_location` infers no loader from
    a suffix it does not recognise -- which made the installer report a missing
    matcher while the file sat right next to it.
    """
    for path in PLACEMENT_CANDIDATES:
        if not path.is_file():
            continue
        loader = importlib.machinery.SourceFileLoader("place_by_capability", str(path))
        spec = importlib.util.spec_from_loader(loader.name, loader)
        module = importlib.util.module_from_spec(spec)
        try:
            loader.exec_module(module)
        except Exception as problem:  # a matcher that cannot load must not place
            return NONE, f"{path} did not load: {problem}"
        return module, str(path)
    named = " ".join(str(path) for path in PLACEMENT_CANDIDATES)
    return NONE, f"no placement matcher is installed at any of: {named}"


def placement(module, requires, max_stale_seconds):
    """Return (host, refusal lines). Any failure to ask is itself a refusal."""
    try:
        return module.place(requires, max_stale_seconds)
    except module.Unreadable as problem:
        return NONE, [f"(every candidate)  {problem}"]


def source_document(label):
    # The delivered copy comes last: a unit already installed on this host is
    # the truth about this host, and a delivery only matters where the job has
    # never run, which is exactly the host placement has just chosen.
    for path in (AGENTS / f"{label}.plist", DAEMONS / f"{label}.plist",
                 AGENTS / f"{label}.plist.awaiting-graphical-session",
                 AGENTS / f"{label}.plist.superseded-by-system-daemon",
                 DELIVERED / f"{label}.plist"):
        if path.is_file():
            try:
                return path, plistlib.loads(path.read_bytes())
            except (OSError, ValueError):
                continue
    return NONE, NONE


def trajectory_of(label, document):
    """Derive the trajectory this unit runs, and say how it was derived.

    These units do not name the trajectory directly: they exec a per-vendor
    `reauth-launch.sh` that sources the deployment environment and then runs
    `scripts/trajectories/<vendor>/reauth.mjs`. Following the launcher is what
    keeps the requirement attached to the work; the label table below it is a
    guess kept only for a host where the launcher is not on disk.
    """
    arguments = [str(item) for item in document.get("ProgramArguments", [])]
    if document.get("Program"):
        arguments.append(str(document["Program"]))
    for argument in arguments:
        found = TRAJECTORY_IN_PATH.search(argument)
        if found:
            return found.group(FIRST), "named by the unit"
    for argument in arguments:
        launcher = pathlib.Path(argument)
        if not launcher.is_file():
            continue
        try:
            found = TRAJECTORY_IN_PATH.search(launcher.read_text(encoding="utf-8", errors="replace"))
        except OSError:
            continue
        if found:
            return found.group(FIRST), f"read out of {launcher}"
    guess = TRAJECTORY_OF.get(label)
    if guess:
        return guess, "guessed from the label; the launcher is not readable here"
    return NONE, "no trajectory could be derived from this unit"


def shaped(document, as_daemon):
    wanted = dict(document)
    label = wanted.get("Label", "reauth")
    environment = dict(wanted.get("EnvironmentVariables", {}))
    environment.setdefault("HOME", str(HOME))
    environment.setdefault("PATH", "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin")
    wanted["EnvironmentVariables"] = environment
    wanted.setdefault("WorkingDirectory", str(HOME))
    wanted.setdefault("StandardOutPath", str(LOGS / f"{label}.out"))
    wanted.setdefault("StandardErrorPath", str(LOGS / f"{label}.err"))
    if as_daemon:
        wanted["UserName"] = os.environ.get("USER", HOME.name)
        wanted.pop("LimitLoadToSessionType", NONE)
        return wanted
    # An agent inherits the user; naming the session type is what keeps it out
    # of the login-less domain this repair is about.
    wanted.pop("UserName", NONE)
    wanted["LimitLoadToSessionType"] = "Aqua"
    return wanted


def already_installed(path, document, domain, label):
    """A loaded job whose unit already matches is left alone.

    Re-bootstrapping a healthy daemon means a bootout that can succeed while the
    bootstrap that follows fails, and the job the operator came to keep is gone.
    """
    if not path.is_file():
        return NONE
    try:
        if plistlib.loads(path.read_bytes()) != document:
            return NONE
    except (OSError, ValueError):
        return NONE
    return state(domain, label)


def can_elevate():
    """Whether this session can act on the system domain without a password."""
    proc = subprocess.run(
        ["/usr/bin/sudo", "-n", "/usr/bin/true"], capture_output=True, text=True, check=False
    )
    return proc.returncode == ZERO


def install_daemon(label, document):
    path = DAEMONS / f"{label}.plist"
    settled = already_installed(path, document, "system", label)
    if settled:
        return settled, "unit already matches; left loaded"
    loaded = state("system", label)
    if not can_elevate():
        # A bootout that succeeds followed by a bootstrap that cannot elevate
        # leaves the operator with no job at all. Refuse the whole exchange.
        return loaded, "this session has no passwordless sudo; the system domain was not touched"
    staging = HOME / ".stado" / "files" / f"{label}.plist"
    staging.parent.mkdir(parents=True, exist_ok=True)
    with staging.open("wb") as handle:
        plistlib.dump(document, handle)
    os.chmod(staging, OWNER_ONLY)
    run("/usr/bin/sudo", "-n", "/bin/cp", str(staging), str(path))
    run("/usr/bin/sudo", "-n", "/usr/sbin/chown", "root:wheel", str(path))
    run("/usr/bin/sudo", "-n", "/bin/chmod", "u=rw,go=r", str(path))
    run("/usr/bin/sudo", "-n", "/bin/launchctl", "bootout", f"system/{label}")
    time.sleep(len("a"))
    run("/usr/bin/sudo", "-n", "/bin/launchctl", "enable", f"system/{label}")
    detail = run("/usr/bin/sudo", "-n", "/bin/launchctl", "bootstrap", "system", str(path))
    return state("system", label), detail.strip()


def install_agent(label, document):
    AGENTS.mkdir(parents=True, exist_ok=True)
    path = AGENTS / f"{label}.plist"
    settled = already_installed(path, document, f"gui/{UID}", label)
    if settled:
        return settled, "unit already matches; left loaded"
    with path.open("wb") as handle:
        plistlib.dump(document, handle)
    os.chmod(path, 0o644)
    run("/bin/launchctl", "bootout", f"gui/{UID}/{label}")
    time.sleep(len("a"))
    run("/bin/launchctl", "enable", f"gui/{UID}/{label}")
    detail = run("/bin/launchctl", "bootstrap", f"gui/{UID}", str(path))
    return state(f"gui/{UID}", label), detail.strip()


def stage_agent(label, document):
    """Keep the shaped unit on disk without loading it, so the next run is a load."""
    AGENTS.mkdir(parents=True, exist_ok=True)
    kept = AGENTS / f"{label}.plist.awaiting-graphical-session"
    with kept.open("wb") as handle:
        plistlib.dump(document, handle)
    return kept


def retire_daemon(label):
    path = DAEMONS / f"{label}.plist"
    if not path.is_file():
        return "no system daemon"
    run("/usr/bin/sudo", "-n", "/bin/launchctl", "bootout", f"system/{label}")
    run("/usr/bin/sudo", "-n", "/bin/rm", "-f", str(path))
    return "system daemon removed"


def deliver(host, label, document):
    """Hand the placed host the unit itself, since the job has never run there."""
    if not STADO.is_file():
        return [f"{STADO} is absent here, so {label} cannot reach {host}"]
    DELIVERED.mkdir(parents=True, exist_ok=True)
    staging = DELIVERED / f"{label}.plist"
    with staging.open("wb") as handle:
        plistlib.dump(document, handle)
    os.chmod(staging, OWNER_ONLY)
    return [
        line
        for line in run(
            str(STADO), "host", "install-file", host, str(staging), f"{label}.plist"
        ).splitlines()
        if line.strip()
    ]


def delegate(host):
    """Run this same installer on the host placement named.

    This cannot bounce between hosts: the placed host asks the same question and
    is its own answer, so it installs locally and delegates to nobody. It also
    cannot be handed operator words -- `run-helper` passes none -- which is why
    the unit travels as a file and the installer re-derives everything there.
    """
    if not STADO.is_file():
        return [f"{STADO} is absent here, so the placed host cannot be reached"]
    source = pathlib.Path(__file__).resolve()
    transfer = run(str(STADO), "host", "install-helper", host, str(source), HELPER_NAME)
    matcher = ""
    beside = next((path for path in PLACEMENT_CANDIDATES if path.is_file()), NONE)
    if beside is not NONE:
        # The placed host must be able to ask the same question this host asked,
        # or its own run refuses for want of a matcher.
        matcher = run(
            str(STADO), "host", "install-helper", host, str(beside), "place-by-capability"
        )
    executed = run(str(STADO), "host", "run-helper", host, HELPER_NAME)
    return [line for line in (transfer + matcher + executed).splitlines() if line.strip()]


def main():
    parser = argparse.ArgumentParser(
        description="Place every reauth job in the launchd domain and on the host its work needs."
    )
    parser.add_argument(
        "--max-stale-seconds",
        type=float,
        default=MAX_STALE_SECONDS,
        help="capability measurements older than this satisfy nothing",
    )
    arguments = parser.parse_args()

    LOGS.mkdir(parents=True, exist_ok=True)
    graphical = graphical_domain_exists()
    here = self_target()
    source, declared = requirements_table()
    matcher, matcher_note = placement_module()

    declarations = {}
    # The matcher is installed as its own helper, so the two can be different
    # ages on the same host. Ask what this copy can answer instead of assuming.
    reader = getattr(matcher, "registry_declarations", NONE) if matcher is not NONE else NONE
    if reader is NONE and matcher is not NONE:
        print("registry         this matcher predates registry_declarations; "
              "the placed host's platform cannot be checked")
    elif reader is not NONE:
        try:
            declarations = reader()
        except matcher.Unreadable as problem:
            # Losing the registry costs the platform check below, not the run.
            print(f"registry         unreadable: {problem}")

    print(f"caller session   {run('/bin/launchctl', 'managername').strip()}")
    print(f"gui/{UID} domain  {'present' if graphical else 'absent -- nobody is logged in on this host'}")
    print(f"this host        {here or 'not a registry target (stado registry self said nothing)'}  "
          f"{platform.system()} {platform.machine()}")
    print(f"requirements     {source}")
    print(f"matcher          {matcher_note}")
    print(f"staleness window {arguments.max_stale_seconds:.0f}s")
    if platform.system() != "Darwin":
        # These units are launchd plists. On any other system this installer has
        # nothing true to do, and staging plists there would leave a declaration
        # nothing can ever load.
        print(f"refused          this host runs {platform.system()}; these are launchd units and "
              "only macOS loads them")
        return NONE

    elsewhere = []
    for label in TRAJECTORY_OF:
        unit, document = source_document(label)
        if not document:
            print(f"== {label}")
            print("   no unit file anywhere; nothing to place")
            continue
        trajectory, derivation = trajectory_of(label, document)
        # An undeclared trajectory is not a trajectory with no needs. The Weles
        # runtime guard throws on one rather than defaulting, and a placement
        # that defaults would put the browser job back in `Background`.
        requires = declared.get(trajectory, FALLBACK_REQUIREMENTS.get(trajectory))
        needs = ",".join(requires) if requires else "nothing measured"
        print(f"== {label}  runs {trajectory or '(unknown)'} ({derivation})  needs "
              f"{needs if requires is not NONE else 'undeclared'}")
        print(f"   source       {unit}")
        if requires is NONE:
            print(f"   refused      {trajectory or label} is declared in no requirements table; "
                  "nothing may be placed from a guess")
            continue

        if not requires:
            # Nothing about this work constrains the host, so the host that owns
            # the schedule owns the job, and `system` is where it belongs.
            loaded, detail = install_daemon(label, shaped(document, as_daemon=True))
            print(f"   system       {loaded or 'not loaded'} {detail if not loaded else detail}")
            continue

        if matcher is NONE:
            print(f"   refused      {matcher_note}")
            print(f"   staged       {stage_agent(label, shaped(document, as_daemon=False))}")
            continue

        host, refusals = placement(matcher, requires, arguments.max_stale_seconds)
        if host is NONE:
            print("   refused      no host published measurements satisfying this job:")
            for line in refusals:
                print(f"     {line}")
            print(f"   retired      {retire_daemon(label)}")
            print(f"   staged       {stage_agent(label, shaped(document, as_daemon=False))}")
            continue

        print(f"   placed       {host}")
        for line in refusals:
            print(f"     not {line}")
        print(f"   retired      {retire_daemon(label)}")
        # Placement answers with the host the work belongs on, and that is no
        # longer always a Mac: once the fleet's browser work moves to Linux under
        # a virtual display, the right answer is a host that cannot load a
        # launchd plist at all. Say that instead of shipping a plist to it.
        shape = (declarations.get(host, {}).get("release_platform") or "").strip()
        if shape and not shape.startswith("darwin"):
            print(f"   refused      {host} is the right host and the wrong shape: its registry "
                  f"release_platform is {shape}, and this unit is a launchd plist, which only "
                  "macOS loads; that trajectory needs a systemd unit there")
            print(f"   staged       {stage_agent(label, shaped(document, as_daemon=False))}")
            continue
        if host != here:
            # The job belongs on another machine, and it has never run there, so
            # the unit travels with the decision. Staging a copy here as well
            # would leave it one login away from running in the wrong place.
            stray = AGENTS / f"{label}.plist.awaiting-graphical-session"
            if stray.is_file():
                stray.unlink()
            elsewhere.append(host)
            print(f"   delegated    {host} will install it; delivering the unit there")
            for line in deliver(host, label, shaped(document, as_daemon=False)):
                print(f"     {line}")
            continue
        if not graphical:
            # The measurement says this host can own a window and launchd says
            # there is no graphical domain to bootstrap into. Believe the domain
            # and say the contradiction out loud rather than install a job that
            # will never load.
            print(f"   refused      {host} measured display true but gui/{UID} does not exist here")
            print(f"   staged       {stage_agent(label, shaped(document, as_daemon=False))}")
            continue
        loaded, detail = install_agent(label, shaped(document, as_daemon=False))
        print(f"   gui/{UID}     {loaded or 'not loaded'} {detail}")

    for host in sorted(set(elsewhere)):
        print(f"== {host}")
        for line in delegate(host):
            print(f"   {line}")
    return NONE


sys.exit(main())
