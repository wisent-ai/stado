#!/usr/bin/env python3
"""Make the tailnet object proxy pick up new TLS material, without root.

`load-tailnet-object-proxy` bootstraps the unit; it cannot reload a unit that is
already loaded, and `launchctl bootstrap` on a live label answers
`Bootstrap failed: 5: Input/output error` while the old process keeps serving the
old certificate. Kickstarting a system-domain label needs root, which no helper
here has and which would mean putting an admin password on this host -- the one
place a host account must never be.

The unit does not need any of that. It declares `UserName charles` and
`KeepAlive true`, so the process runs as the account this helper already is, and
launchd respawns it the moment it exits. Terminating it is therefore the reload,
and it needs no privilege at all.

Verified rather than assumed: this records the process before, terminates it, waits
for the listener to answer again, and then completes a real TLS handshake against
the host's own anchor, reporting the leaf it was served. A proxy that came back
holding the old certificate is a failure this reports instead of hiding.
"""

import datetime
import hashlib
import os
import pathlib
import signal
import socket
import ssl
import subprocess
import sys
import time

NONE = None
ZERO = len([])
ONE = len("a")
SHORT = len("a" * 12)
HOME = pathlib.Path(os.path.expanduser("~"))
ANCHOR = HOME / ".stado" / "stado-tailnet-ca.crt"
LAUNCHER = "start-stado-tailnet-object-proxy"
# The launcher is what launchd runs; the program is what the launcher execs, and
# on a running host only the latter is visible in `ps`.
PROGRAM = "stado-tailnet-object-proxy"
TAILNET_IP = "100.120.25.24"
TAILNET_DNS = "charless-mac-mini.tail6443b3.ts.net"
PORT = 8765
# launchd's respawn is prompt, but the node process has to bind before a caller
# succeeds, so poll rather than sleep once and hope.
PATIENCE = len("a" * 30)
ENVIRONMENT = {
    **os.environ,
    "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
}


def run(*args):
    try:
        return subprocess.run(
            args, capture_output=True, text=True, check=False, env=ENVIRONMENT
        )
    except FileNotFoundError:
        return subprocess.CompletedProcess(args, ONE, "", f"{args[ZERO]} not installed")


def digest(text):
    return hashlib.sha256(text.encode()).hexdigest()[:SHORT]


def serving_pids():
    """Only this proxy's own processes -- its launcher and the node it runs.

    Selecting by port would be wrong and was: the loopback object API listens on
    the same port number on this host, so a port-wide sweep terminates the store
    the proxy exists to publish. The command line is what identifies the proxy.
    """
    pids = {}
    processes = run("ps", "-Ao", "pid,command")
    for line in processes.stdout.splitlines():
        if "ps -Ao" in line:
            continue
        if LAUNCHER in line or PROGRAM in line:
            columns = line.split()
            pids[columns[ZERO]] = "launcher" if LAUNCHER in line else "proxy"
    return pids


def described(pid):
    return run("ps", "-o", "lstart=,command=", "-p", pid).stdout.strip()


def handshake():
    """One real TLS connection, verified against this host's anchor."""
    context = ssl.create_default_context(cafile=str(ANCHOR))
    try:
        with socket.create_connection((TAILNET_IP, PORT), timeout=len("aaaaaaaaaa")) as raw:
            with context.wrap_socket(raw, server_hostname=TAILNET_DNS) as tls:
                served = tls.getpeercert()
                binary = tls.getpeercert(binary_form=True)
                return (
                    f"trusted, {tls.version()}",
                    served.get("subjectAltName"),
                    served.get("notAfter"),
                    digest(ssl.DER_cert_to_PEM_cert(binary)),
                )
    except ssl.SSLCertVerificationError as refusal:
        return (
            f"REFUSED: {refusal.verify_message or refusal.reason} (code {refusal.verify_code})",
            NONE,
            NONE,
            "",
        )
    except OSError as refusal:
        return (f"unreachable: {refusal}", NONE, NONE, "")


def main():
    if not ANCHOR.is_file():
        raise SystemExit(f"no anchor at {ANCHOR}; re-anchor this host first")
    print(f"host         {run('hostname').stdout.strip()} as {run('id', '-un').stdout.strip()}")
    print(f"anchor       sha256:{digest(ANCHOR.read_text(encoding='utf-8'))}")

    print()
    print("=== before ===")
    before = serving_pids()
    for pid, owner in sorted(before.items()):
        print(f"  pid {pid:<8}{owner:<10}{described(pid)}")
    state, names, expires, served = handshake()
    print(f"  handshake  {state}")
    print(f"  served     SANs {names} expires {expires} leaf sha256:{served or '(none)'}")
    if not before:
        raise SystemExit("nothing is serving the proxy port; use load-tailnet-object-proxy first")

    print()
    print("=== reload ===")
    for pid in sorted(before, key=int):
        try:
            os.kill(int(pid), signal.SIGTERM)
            print(f"  SIGTERM    {pid}")
        except (ProcessLookupError, PermissionError) as refusal:
            print(f"  SIGTERM    {pid} refused: {refusal}")
    deadline = time.monotonic() + PATIENCE
    while time.monotonic() < deadline:
        time.sleep(ONE)
        if serving_pids() and handshake()[ZERO].startswith(("trusted", "REFUSED")):
            break
    print(f"  waited     {'listener answered again' if serving_pids() else 'NOTHING came back'}")

    print()
    print("=== after ===")
    after = serving_pids()
    for pid, owner in sorted(after.items()):
        print(f"  pid {pid:<8}{owner:<10}{described(pid)}")
    state, names, expires, fresh = handshake()
    print(f"  handshake  {state}")
    print(f"  served     SANs {names} expires {expires} leaf sha256:{fresh or '(none)'}")
    print(f"  replaced   {'yes' if fresh and fresh != served else 'NO, same leaf as before'}")
    print(f"  now        {datetime.datetime.now(datetime.timezone.utc).isoformat(timespec='seconds')}")
    return ZERO if state.startswith("trusted") else ONE


sys.exit(main())
