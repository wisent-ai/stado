#!/usr/bin/env python3
"""Put the newly signed server certificate and key into service on this host.

The proxy launcher serves `~/.stado/stado-tailnet-server.crt` with its matching
key, so this replaces exactly those two files from `~/.stado/tls-next` and nothing
else. Both are copied to `.before-<timestamp>` first: a server presenting a
certificate whose key does not match it fails every handshake, and the way back
has to be one `cp` rather than a re-issue.

Three refusals before anything is written, because each of them is a way to take
the endpoint down that a handshake error would not explain: the incoming key must
pair with the incoming certificate, the incoming certificate must verify against
the anchor this host now carries, and it must still cover the addresses the callers
dial. Restarting the proxy is deliberately left to the caller, so the swap and the
restart are separately reversible.

Idempotent: files already identical to the incoming ones are left alone. Prints
digests and public-half digests only; no private key is read into the output.
"""

import datetime
import hashlib
import os
import pathlib
import shutil
import subprocess
import sys

NONE = None
ZERO = len([])
ONE = len("a")
SHORT = len("a" * 12)
HOME = pathlib.Path(os.path.expanduser("~"))
STADO_DIR = HOME / ".stado"
SCRATCH = STADO_DIR / "tls-next"
ANCHOR = STADO_DIR / "stado-tailnet-ca.crt"
LIVE_CERT = STADO_DIR / "stado-tailnet-server.crt"
LIVE_KEY = STADO_DIR / "stado-tailnet-server.key"
NEW_CERT = SCRATCH / "stado-tailnet-server.crt"
NEW_KEY = SCRATCH / "stado-tailnet-server.key"
# What the fleet's configs dial, so a certificate that stopped covering them is
# refused here rather than discovered by a caller.
REQUIRED_NAMES = ["100.120.25.24", "control-host.tail6443b3.ts.net"]
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


def public_of_certificate(path):
    return digest(run("openssl", "x509", "-in", str(path), "-noout", "-pubkey").stdout)


def public_of_key(path):
    derived = run("openssl", "pkey", "-in", str(path), "-pubout")
    return digest(derived.stdout) if derived.returncode == ZERO else ""


def names_in(path):
    text = run("openssl", "x509", "-in", str(path), "-noout", "-text").stdout
    lines = text.splitlines()
    for index, line in enumerate(lines):
        if "X509v3 Subject Alternative Name" in line:
            return lines[index + ONE].strip() if index + ONE < len(lines) else ""
    return ""


def show(label, path):
    if not path.is_file():
        print(f"  {label:<10}{path} ABSENT")
        return
    body = path.read_bytes()
    print(
        f"  {label:<10}{path.name}  sha256:{digest(body.decode('utf-8', 'replace'))}  "
        f"{len(body)} bytes  mode {oct(path.stat().st_mode & 0o777)}"
    )


def main():
    os.umask(0o077)
    print(f"host         {run('hostname').stdout.strip()}")
    for path in (NEW_CERT, NEW_KEY, ANCHOR):
        if not path.is_file():
            raise SystemExit(f"missing {path}; mint the authority first")

    print()
    print("=== incoming ===")
    show("cert", NEW_CERT)
    show("key", NEW_KEY)
    print(f"  pairs     {public_of_certificate(NEW_CERT) == public_of_key(NEW_KEY)}")
    verified = run("openssl", "verify", "-CAfile", str(ANCHOR), str(NEW_CERT))
    print(f"  verify    {verified.stdout.strip() or verified.stderr.strip()}")
    covered = names_in(NEW_CERT)
    print(f"  SANs      {covered or 'ABSENT'}")
    if public_of_certificate(NEW_CERT) != public_of_key(NEW_KEY):
        raise SystemExit("the incoming key does not pair with the incoming certificate")
    if verified.returncode != ZERO:
        raise SystemExit("the incoming certificate does not verify against this host's anchor")
    missing = [name for name in REQUIRED_NAMES if name not in covered]
    if missing:
        raise SystemExit(f"the incoming certificate does not cover {', '.join(missing)}")

    print()
    print("=== in service before ===")
    show("cert", LIVE_CERT)
    show("key", LIVE_KEY)
    if (
        LIVE_CERT.is_file()
        and LIVE_KEY.is_file()
        and LIVE_CERT.read_bytes() == NEW_CERT.read_bytes()
        and LIVE_KEY.read_bytes() == NEW_KEY.read_bytes()
    ):
        print()
        print("settled      the incoming material is already in service")
        return NONE

    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    print()
    for live in (LIVE_CERT, LIVE_KEY):
        if live.is_file():
            backup = live.with_name(f"{live.name}.before-{stamp}")
            shutil.copy2(live, backup)
            print(f"backup       {backup}")
    for source, live in ((NEW_CERT, LIVE_CERT), (NEW_KEY, LIVE_KEY)):
        shutil.copyfile(source, live)
        live.chmod(0o600)

    print()
    print("=== in service after ===")
    show("cert", LIVE_CERT)
    show("key", LIVE_KEY)
    print(f"  pairs     {public_of_certificate(LIVE_CERT) == public_of_key(LIVE_KEY)}")
    print()
    print("restart      the proxy still serves the old material until it is reloaded:")
    print("             stado host run-helper control-host load-tailnet-object-proxy")
    return NONE


sys.exit(main())
