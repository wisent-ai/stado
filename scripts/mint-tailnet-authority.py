#!/usr/bin/env python3
"""Mint the fleet's tailnet certificate authority on the host that will serve it.

The previous authority was minted by hand on this host and its private key was
never kept: an exact search of 845 candidate files across all three fleet hosts
found nothing that pairs with `CN=Stado Tailnet Queue CA`, and no vault item names
it. That is why this is a replacement rather than a repair, and it is the failure
this script exists to stop repeating -- the private key it creates goes into
Skarbiec in the same run that creates it, as item `stado-tailnet-ca`, so the next
agent reads an item id instead of scanning a filesystem with openssl.

It also fixes what made the old anchor Mac-only. A CA certificate that omits
basicConstraints and keyUsage is malformed under current rules: macOS accepts it
and OpenSSL refuses it as `CA cert does not include key usage extension`, so for
months the tailnet store answered exactly one operating system. The new anchor
carries both, critical, and the leaf carries the SANs the callers actually dial.

Run as a Stado helper on the host that owns the key. It generates into
`~/.stado/tls-next`, verifies the chain there, stores the authority in the vault,
and prints the anchor -- a public certificate -- on stdout so an operator can
distribute it. It installs nothing: the swap is a separate, reversible step.

Idempotent: an authority already present in `~/.stado/tls-next` is reused, never
regenerated, because minting a second authority silently orphans the first.
"""

import datetime
import hashlib
import json
import os
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
ONE = len("a")
SHORT = len("a" * 12)
HOME = pathlib.Path(os.path.expanduser("~"))
STADO_DIR = HOME / ".stado"
SKARBIEC = STADO_DIR / "bin" / "skarbiec"
VAULT = STADO_DIR / "skarbiec.vault.json"
SCRATCH = STADO_DIR / "tls-next"
ITEM = "stado-tailnet-ca"
TAGS = "fleet:tailnet-tls"
# The addresses the callers dial, taken from the certificate this replaces so the
# leaf keeps covering exactly what the configs already name.
TAILNET_IP = "100.120.25.24"
TAILNET_DNS = "control-host.tail6443b3.ts.net"
CA_SUBJECT = "/CN=Stado Tailnet Queue CA"
LEAF_SUBJECT = f"/CN={TAILNET_DNS}"
# Ten years for the authority, matching the one it replaces, and the longest life
# a public authority may issue for the leaf, so a habit built here stays valid if
# this ever moves to a publicly trusted issuer.
CA_DAYS = "3650"
LEAF_DAYS = "825"
CA_BITS = "4096"
LEAF_BITS = "2048"
ENVIRONMENT = {
    **os.environ,
    "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
}

# An extension file rather than -addext: LibreSSL ships as /usr/bin/openssl on
# macOS and its -addext support has differed by release, while -extfile has been
# portable for twenty years.
CA_EXTENSIONS = """[ca]
basicConstraints=critical,CA:TRUE
keyUsage=critical,keyCertSign,cRLSign
subjectKeyIdentifier=hash
"""
LEAF_EXTENSIONS = f"""[leaf]
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth
subjectKeyIdentifier=hash
authorityKeyIdentifier=keyid,issuer
subjectAltName=IP:{TAILNET_IP},DNS:{TAILNET_DNS}
"""


def run(*args, stdin=NONE):
    try:
        return subprocess.run(
            args,
            capture_output=True,
            text=True,
            check=False,
            input=stdin,
            env={**ENVIRONMENT, "SKARBIEC_VAULT_FILE": str(VAULT)},
        )
    except FileNotFoundError:
        return subprocess.CompletedProcess(args, ONE, "", f"{args[ZERO]} not installed")


def must(result, what):
    if result.returncode != ZERO:
        raise SystemExit(f"{what} failed: {result.stderr.strip().splitlines()[-ONE:]}")
    return result


def digest(text):
    return hashlib.sha256(text.encode()).hexdigest()[:SHORT]


def public_digest(path, kind):
    """A key or certificate identified by the digest of its public half only."""
    if kind == "key":
        derived = must(run("openssl", "pkey", "-in", str(path), "-pubout"), f"read {path.name}")
    else:
        derived = must(
            run("openssl", "x509", "-in", str(path), "-noout", "-pubkey"), f"read {path.name}"
        )
    return digest(derived.stdout)


def extensions_of(path):
    text = must(
        run("openssl", "x509", "-in", str(path), "-noout", "-text"), f"parse {path.name}"
    ).stdout
    lines = text.splitlines()
    wanted = {}
    for needle, name in (
        ("X509v3 Basic Constraints", "basicConstraints"),
        ("X509v3 Key Usage", "keyUsage"),
        ("X509v3 Extended Key Usage", "extendedKeyUsage"),
        ("X509v3 Subject Alternative Name", "subjectAltName"),
    ):
        wanted[name] = ""
        for index, line in enumerate(lines):
            if needle in line:
                value = line.split(":", ONE)[-ONE].strip()
                following = lines[index + ONE].strip() if index + ONE < len(lines) else ""
                wanted[name] = following if value in ("", "critical") else value
                if value == "critical":
                    wanted[name] += " (critical)"
                break
    return wanted


def main():
    # The helper runs under a umask that strips group and other entirely, because
    # a private key must never be readable by anything but its owner.
    os.umask(0o077)
    SCRATCH.mkdir(parents=True, exist_ok=True)
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    ca_key, ca_crt = SCRATCH / "stado-tailnet-ca.key", SCRATCH / "stado-tailnet-ca.crt"
    leaf_key = SCRATCH / "stado-tailnet-server.key"
    leaf_csr = SCRATCH / "stado-tailnet-server.csr"
    leaf_crt = SCRATCH / "stado-tailnet-server.crt"
    ca_cnf, leaf_cnf = SCRATCH / "ca.cnf", SCRATCH / "leaf.cnf"
    ca_cnf.write_text(CA_EXTENSIONS, encoding="utf-8")
    leaf_cnf.write_text(LEAF_EXTENSIONS, encoding="utf-8")

    print(f"host         {run('hostname').stdout.strip()}")
    print(f"openssl      {run('openssl', 'version').stdout.strip()}")
    print(f"scratch      {SCRATCH}  mode {oct(SCRATCH.stat().st_mode & 0o777)}")
    print(f"stamp        {stamp}")

    if ca_key.is_file() and ca_crt.is_file():
        print("authority    reusing the authority already in the scratch directory")
    else:
        must(run("openssl", "genrsa", "-out", str(ca_key), CA_BITS), "generate CA key")
        must(
            run(
                "openssl", "req", "-x509", "-new", "-key", str(ca_key),
                "-sha256", "-days", CA_DAYS, "-subj", CA_SUBJECT,
                "-extensions", "ca", "-config", str(ca_cnf),
                "-out", str(ca_crt),
            ),
            "self-sign CA certificate",
        )
        print(f"authority    minted a new {CA_BITS}-bit authority valid {CA_DAYS} days")

    if not (leaf_key.is_file() and leaf_crt.is_file()):
        must(run("openssl", "genrsa", "-out", str(leaf_key), LEAF_BITS), "generate leaf key")
        must(
            run(
                "openssl", "req", "-new", "-key", str(leaf_key),
                "-subj", LEAF_SUBJECT, "-out", str(leaf_csr),
            ),
            "create leaf request",
        )
        must(
            run(
                "openssl", "x509", "-req", "-in", str(leaf_csr),
                "-CA", str(ca_crt), "-CAkey", str(ca_key), "-CAcreateserial",
                "-sha256", "-days", LEAF_DAYS,
                "-extfile", str(leaf_cnf), "-extensions", "leaf",
                "-out", str(leaf_crt),
            ),
            "sign leaf certificate",
        )
        print(f"leaf         signed a {LEAF_BITS}-bit server certificate for {LEAF_DAYS} days")
    else:
        print("leaf         reusing the server certificate already in the scratch directory")
    for path in (ca_key, leaf_key):
        path.chmod(0o600)

    print()
    print("=== what was produced ===")
    for label, path in (("ca", ca_crt), ("leaf", leaf_crt)):
        marks = extensions_of(path)
        print(f"  {label:<5}{path.name}")
        print(f"       pubkey  sha256:{public_digest(path, 'cert')}")
        for name, value in marks.items():
            print(f"       {name:<17}{value or 'ABSENT'}")
    print(f"  ca_key pairs_with_ca  {public_digest(ca_key, 'key') == public_digest(ca_crt, 'cert')}")
    print(f"  leaf_key pairs_with_leaf  {public_digest(leaf_key, 'key') == public_digest(leaf_crt, 'cert')}")

    print()
    print("=== chain verification on the host that will serve it ===")
    verified = run("openssl", "verify", "-CAfile", str(ca_crt), str(leaf_crt))
    print(f"  openssl verify  {verified.stdout.strip() or verified.stderr.strip()}")
    if verified.returncode != ZERO:
        raise SystemExit("the new chain does not verify; nothing was stored")

    print()
    print("=== authority stored in Skarbiec ===")
    payload = {
        "schema": "skarbiec.item.v2",
        "kind": "certificate",
        "fields": {
            "certificate": ca_crt.read_text(encoding="utf-8"),
            "private_key": ca_key.read_text(encoding="utf-8"),
        },
        "context": {
            "source_kind": "fleet-tls",
            "name": ITEM,
            "domains": f"{TAILNET_IP},{TAILNET_DNS}",
            "operation": (
                "signs the tailnet object proxy certificate on control-host; "
                "anchored by control-host, operator-host and "
                "gpu-host as storage.stado.ca_file"
            ),
        },
    }
    stored = run(str(SKARBIEC), "set-json", ITEM, "--tags", TAGS, stdin=json.dumps(payload))
    if stored.returncode != ZERO:
        raise SystemExit(f"vault write refused: {stored.stderr.strip().splitlines()[-ONE:]}")
    print(f"  vault      {VAULT}")
    print(f"  {stored.stdout.strip()}")
    print(f"  key_digest sha256:{public_digest(ca_key, 'key')} (public half only)")

    print()
    print("=== anchor to distribute (public certificate) ===")
    print(ca_crt.read_text(encoding="utf-8").strip())
    return NONE


sys.exit(main())
