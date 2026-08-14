#!/usr/bin/env python3
"""Inventory this host's tailnet TLS material before anyone re-issues it.

`report-tailnet-tls.py` asks the caller's question -- does this host trust the
endpoint it dials. This asks the operator's question, which is different and has
to be answered first: which files on this host are the trust material, what is
actually inside them, and which units and scripts here would have to receive a
replacement. A certificate authority swap that misses one consumer takes that
consumer offline silently, and the failure surfaces as an unreachable host rather
than an untrusted one.

So this prints, for the anchor, the server certificate and any private key beside
them: presence, size, mode, a certificate fingerprint, the subject and issuer, the
validity window, and the three extensions that decide whether OpenSSL will accept
the anchor at all -- basicConstraints, keyUsage and extendedKeyUsage. A CA
certificate that omits basicConstraints and keyUsage is malformed under current
rules; macOS tolerates it and OpenSSL does not, which is exactly how a fleet
becomes accidentally Mac-only.

Read-only, and no private key is ever read or printed: a key is measured only by
the digest of the public half openssl derives from it, which is what proves it
matches a certificate without disclosing anything.
"""

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
CONFIG = pathlib.Path(os.environ.get("STADO_CONFIG", HOME / ".config" / "stado" / "config.json"))
ANCHOR = STADO_DIR / "stado-tailnet-ca.crt"
SERVER = STADO_DIR / "stado-tailnet-server.crt"
# Every place a launcher, unit or helper on this host could name the material.
UNIT_ROOTS = [
    STADO_DIR / "bin",
    HOME / "Library" / "LaunchAgents",
    pathlib.Path("/Library/LaunchDaemons"),
    pathlib.Path("/etc/systemd/system"),
    HOME / ".config" / "systemd" / "user",
]
NAMES = ["stado-tailnet-ca", "stado-tailnet-server", "stado-tailnet"]
ENVIRONMENT = {
    **os.environ,
    "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
}


def run(*args, stdin=NONE, vault=NONE):
    environment = dict(ENVIRONMENT)
    if vault:
        environment["SKARBIEC_VAULT_FILE"] = str(vault)
    try:
        return subprocess.run(
            args,
            capture_output=True,
            text=True,
            check=False,
            input=stdin,
            env=environment,
        )
    except FileNotFoundError:
        return subprocess.CompletedProcess(args, ONE, "", f"{args[ZERO]} not installed")


def digest(text):
    return hashlib.sha256(text.encode()).hexdigest()[:SHORT]


def field(text, needle):
    """One openssl -text value, joined with its continuation line when it has one.

    An extension prints as `X509v3 Key Usage: critical` with the value on the next
    line, so taking only the labelled line reports `critical` and loses the part
    that matters -- whether the usage actually includes keyCertSign.
    """
    lines = text.splitlines()
    for index, line in enumerate(lines):
        if needle not in line:
            continue
        rest = line.split(":", ONE)[-ONE].strip()
        following = lines[index + ONE].strip() if index + ONE < len(lines) else ""
        if rest and rest != "critical":
            return rest
        return f"{following} (critical)" if rest == "critical" else following
    return ""


def describe_file(path):
    if not path.is_file():
        print(f"  {path}  ABSENT")
        return NONE
    facts = path.stat()
    print(f"  {path}")
    print(f"    size       {facts.st_size} bytes   mode {oct(facts.st_mode & 0o777)}")
    return facts


def describe_certificate(path, label):
    print(f"=== {label} ===")
    if describe_file(path) is NONE:
        return NONE
    text = run("openssl", "x509", "-in", str(path), "-noout", "-text").stdout
    if not text:
        print("    parse      openssl could not read this as a certificate")
        return NONE
    fingerprint = run("openssl", "x509", "-in", str(path), "-noout", "-fingerprint", "-sha256")
    public = run("openssl", "x509", "-in", str(path), "-noout", "-pubkey")
    print(f"    subject    {field(text, 'Subject:')}")
    print(f"    issuer     {field(text, 'Issuer:')}")
    print(f"    serial     {field(text, 'Serial Number')}")
    print(f"    validity   {field(text, 'Not Before')} -> {field(text, 'Not After')}")
    print(f"    {fingerprint.stdout.strip() or 'fingerprint unavailable'}")
    print(f"    pubkey_sha sha256:{digest(public.stdout)}")
    # The three extensions that decide whether OpenSSL accepts this as an anchor.
    for needle, name in (
        ("X509v3 Basic Constraints", "basicConstraints"),
        ("X509v3 Key Usage", "keyUsage"),
        ("X509v3 Extended Key Usage", "extendedKeyUsage"),
        ("X509v3 Subject Alternative Name", "subjectAltName"),
    ):
        print(f"    {name:<17}{field(text, needle) or 'ABSENT'}")
    return digest(public.stdout)


def describe_key(path, expected_public):
    print(f"=== private key {path.name} ===")
    if describe_file(path) is NONE:
        return
    # Only the public half is derived, and only its digest is printed.
    public = run("openssl", "pkey", "-in", str(path), "-pubout")
    if public.returncode != ZERO:
        print(f"    parse      openssl refused this key: {public.stderr.strip()[:SHORT * len('aaa')]}")
        return
    print(f"    pubkey_sha sha256:{digest(public.stdout)}")
    if expected_public:
        print(
            f"    pairs_with {'YES' if digest(public.stdout) == expected_public else 'NO'}"
        )


def main():
    print(f"host         {run('hostname').stdout.strip()} as {run('id', '-un').stdout.strip()}")
    print(f"home         {HOME}")
    print()

    print("=== stado configuration ===")
    if CONFIG.is_file():
        document = json.loads(CONFIG.read_text(encoding="utf-8"))
        stado = document.get("storage", {}).get("stado", {})
        print(f"  config     {CONFIG}")
        print(f"  url        {stado.get('url') or '(unset)'}")
        print(f"  ca_file    {stado.get('ca_file') or '(unset)'}")
        print(f"  needs_ca   {'yes' if str(stado.get('url', '')).startswith('https://') else 'no'}")
    else:
        print(f"  config     {CONFIG} ABSENT")
    print()

    anchor_public = describe_certificate(ANCHOR, "trust anchor")
    print()
    server_public = describe_certificate(SERVER, "server certificate")
    print()

    print("=== every tailnet-named file in the stado directory ===")
    found = sorted(
        path
        for path in STADO_DIR.glob("*tailnet*")
        if path.is_file()
    ) if STADO_DIR.is_dir() else []
    for path in found:
        facts = path.stat()
        print(f"  {path.name:<44} {facts.st_size:>7} bytes  mode {oct(facts.st_mode & 0o777)}")
    if not found:
        print("  (none)")
    print()

    for path in found:
        if path.suffix == ".key" or path.name.endswith("-key.pem"):
            describe_key(
                path,
                anchor_public if "ca" in path.name else server_public,
            )
            print()

    print("=== units and launchers on this host that name the material ===")
    for root in UNIT_ROOTS:
        if not root.is_dir():
            continue
        for candidate in sorted(root.iterdir()):
            if not candidate.is_file():
                continue
            try:
                body = candidate.read_text(encoding="utf-8", errors="ignore")
            except OSError:
                continue
            named = [name for name in NAMES if name in body]
            if named:
                print(f"  {candidate}  names {','.join(sorted(set(named)))}")
    print()

    # The other place a certificate authority's private key can legitimately be is
    # the vault, so look before anyone concludes the key is lost -- and look in
    # every vault this host holds, not just the fleet one, because `stado host
    # vaults` routinely reports several per machine. Ids and kinds only: `list` is
    # envelope metadata and never returns a field value.
    print("=== vault items whose id names the material ===")
    vaults = sorted(
        set(STADO_DIR.glob("*vault*.json")) | set(HOME.glob(".skarbiec*vault*.json"))
    )
    if not vaults:
        print("  (this host holds no vault file)")
    wanted = ("tailnet", "tls", "-ca", "ca-", "cert")
    for vault in vaults:
        listed = run(str(STADO_DIR / "bin" / "skarbiec"), "list", "--all", vault=vault)
        if listed.returncode != ZERO:
            print(f"  {vault.name:<52} unreadable: {listed.stderr.strip()[:SHORT * len('aaaa')]}")
            continue
        rows = sorted(
            (entry["id"], entry["kind"])
            for entry in json.loads(listed.stdout)
            if any(word in entry["id"].lower() for word in wanted)
        )
        print(f"  {vault.name:<52} {len(rows)} matching id(s)")
        for item, kind in rows:
            print(f"      {item:<48} {kind}")
    return NONE


sys.exit(main())
