#!/usr/bin/env python3
"""Re-anchor this host on the fleet's tailnet certificate authority.

An anchor swap that misses one consumer takes that consumer offline silently, and
the failure reads as an unreachable host rather than an untrusted one, so this is
written to be run on every host that holds the anchor -- including hosts whose
`storage.stado.url` is loopback today. An anchor that is correct only until someone
flips a scheme is the same latent defect in a new costume.

It refuses to install an authority that is not usable as one: the certificate has
to carry `basicConstraints CA:TRUE` and a `keyUsage` including certificate signing,
because their absence is exactly what made the previous anchor work on macOS and
fail on Linux with `CA cert does not include key usage extension`. The live anchor
is copied to `stado-tailnet-ca.crt.before-<timestamp>` before anything is written,
so the previous state is one `cp` away.

Idempotent: an anchor already identical to the incoming one is left alone. Prints
digests, subjects and extensions -- a certificate is public by construction and no
private key is read.
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
ANCHOR = STADO_DIR / "stado-tailnet-ca.crt"
# Where the authority is on the host that minted it, and where `stado host
# install-file` lands it on every other host.
SOURCES = [
    STADO_DIR / "tls-next" / "stado-tailnet-ca.crt",
    STADO_DIR / "files" / "stado-tailnet-ca.crt",
]
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


def describe(path):
    text = run("openssl", "x509", "-in", str(path), "-noout", "-text").stdout
    lines = text.splitlines()
    facts = {"parsed": bool(text)}
    for needle, name in (
        ("Subject:", "subject"),
        ("Not After", "expires"),
        ("X509v3 Basic Constraints", "basicConstraints"),
        ("X509v3 Key Usage", "keyUsage"),
    ):
        facts[name] = ""
        for index, line in enumerate(lines):
            if needle in line:
                value = line.split(":", ONE)[-ONE].strip()
                following = lines[index + ONE].strip() if index + ONE < len(lines) else ""
                facts[name] = following if value in ("", "critical") else value
                break
    return facts


def report(label, path):
    if not path.is_file():
        print(f"  {label:<12}{path} ABSENT")
        return NONE
    body = path.read_text(encoding="utf-8")
    facts = describe(path)
    print(f"  {label:<12}{path}")
    print(f"              sha256:{digest(body)}  {path.stat().st_size} bytes  "
          f"mode {oct(path.stat().st_mode & 0o777)}")
    print(f"              {facts['subject']}  expires {facts['expires']}")
    print(f"              basicConstraints {facts['basicConstraints'] or 'ABSENT'}"
          f"   keyUsage {facts['keyUsage'] or 'ABSENT'}")
    return facts


def main():
    os.umask(0o077)
    incoming = next((path for path in SOURCES if path.is_file()), NONE)
    print(f"host         {run('hostname').stdout.strip()} as {run('id', '-un').stdout.strip()}")
    if incoming is NONE:
        raise SystemExit(
            "no incoming anchor: expected "
            + " or ".join(str(path) for path in SOURCES)
        )
    print()
    print("=== incoming ===")
    facts = report("incoming", incoming)
    if not facts or not facts["parsed"]:
        raise SystemExit(f"{incoming} is not a certificate")
    # The two properties that make an anchor an anchor. Refusing here is the whole
    # point: installing a malformed authority is how this fleet became Mac-only.
    if "CA:TRUE" not in facts["basicConstraints"]:
        raise SystemExit(f"{incoming} is not a CA certificate: {facts['basicConstraints']!r}")
    if "Certificate Sign" not in facts["keyUsage"]:
        raise SystemExit(
            f"{incoming} has no certificate-signing keyUsage: {facts['keyUsage']!r}; "
            "OpenSSL refuses such an anchor and macOS does not, which is the bug"
        )

    print()
    print("=== live anchor before ===")
    before = report("live", ANCHOR)
    if ANCHOR.is_file() and digest(ANCHOR.read_text(encoding="utf-8")) == digest(
        incoming.read_text(encoding="utf-8")
    ):
        print()
        print("settled      this host already anchors on the incoming authority")
        return NONE

    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    if ANCHOR.is_file():
        backup = ANCHOR.with_name(f"{ANCHOR.name}.before-{stamp}")
        shutil.copy2(ANCHOR, backup)
        print()
        print(f"backup       {backup}")
    shutil.copyfile(incoming, ANCHOR)
    ANCHOR.chmod(0o600)

    print()
    print("=== live anchor after ===")
    report("live", ANCHOR)
    print()
    print(f"replaced     {'yes' if before else 'installed for the first time'}")
    return NONE


sys.exit(main())
