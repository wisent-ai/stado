#!/usr/bin/env python3
"""Which file on this host, if any, is the private half of a certificate?

Re-issuing a certificate authority is a different job from replacing one: if the
CA private key still exists, the fleet's existing anchor can be repaired and every
certificate under it keeps working, and if it does not, the authority has to be
replaced and every consumer re-anchored. "I could not find it in the obvious
place" is not an answer to that, so this searches and reports what it searched.

The test is exact and needs no secret: openssl derives the public half of each
candidate key and the digest of that public half is compared with the digest of
the certificate's public key. Only paths, digests and counts are printed -- no key
material is ever read into the output, and a candidate that openssl refuses is
counted rather than quoted.

With no argument it asks about the fleet's tailnet authority, so it runs as a
Stado helper; with one argument it asks about that certificate instead.
"""

import hashlib
import os
import pathlib
import subprocess
import sys

NONE = None
ZERO = len([])
ONE = len("a")
SHORT = len("a" * 12)
HOME = pathlib.Path(os.path.expanduser("~"))
DEFAULT_CERT = HOME / ".stado" / "stado-tailnet-ca.crt"
# A PEM private key is small; anything larger is a bundle, a log or a database.
LARGEST = len("a" * 32768)
DEPTH = len("abcd")
ROOTS = [
    HOME / ".stado",
    HOME / ".ssh",
    HOME / "Library" / "Application Support",
    pathlib.Path("/usr/local/etc"),
    pathlib.Path("/opt/homebrew/etc"),
    pathlib.Path("/etc/ssl"),
    pathlib.Path("/tmp"),
    pathlib.Path("/var/tmp"),
    HOME,
]
# Directories that cannot hold fleet TLS material and cost minutes to walk.
SKIP = {
    ".git",
    ".cargo",
    ".rustup",
    ".npm",
    ".cache",
    "node_modules",
    "Caches",
    "recordings",
    "Documents",
    "Downloads",
    "Photos Library.photoslibrary",
    "target",
    "venv",
    ".venv",
    "site-packages",
}
SUFFIXES = (".key", ".pem", ".p8", ".priv")
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


def candidates():
    """Every small, plausibly-PEM file under the bounded roots, once each."""
    seen = set()
    for root in ROOTS:
        if not root.is_dir():
            continue
        base = len(root.parts)
        for directory, children, files in os.walk(root, followlinks=False):
            here = pathlib.Path(directory)
            if len(here.parts) - base >= DEPTH:
                children[:] = []
            children[:] = [
                child
                for child in children
                if child not in SKIP and not child.startswith(".Trash")
            ]
            for name in files:
                path = here / name
                if path in seen:
                    continue
                if not (name.endswith(SUFFIXES) or "key" in name.lower()):
                    continue
                try:
                    if not path.is_file() or path.stat().st_size > LARGEST:
                        continue
                except OSError:
                    continue
                seen.add(path)
                yield path


def main():
    certificate = pathlib.Path(
        sys.argv[ONE] if len(sys.argv) > ONE else DEFAULT_CERT
    ).expanduser()
    print(f"certificate  {certificate}")
    if not certificate.is_file():
        raise SystemExit(f"no certificate at {certificate}")
    public = run("openssl", "x509", "-in", str(certificate), "-noout", "-pubkey")
    if public.returncode != ZERO:
        raise SystemExit(f"openssl could not read {certificate}: {public.stderr.strip()}")
    wanted = digest(public.stdout)
    subject = run("openssl", "x509", "-in", str(certificate), "-noout", "-subject")
    print(f"  {subject.stdout.strip()}")
    print(f"  public half  sha256:{wanted}")
    print(f"  roots        {', '.join(str(root) for root in ROOTS if root.is_dir())}")
    print(f"  depth        {DEPTH} directories below each root")

    scanned = ZERO
    refused = ZERO
    matches = []
    for path in candidates():
        scanned += ONE
        derived = run("openssl", "pkey", "-in", str(path), "-pubout")
        if derived.returncode != ZERO:
            refused += ONE
            continue
        if digest(derived.stdout) == wanted:
            matches.append(path)
    print(f"  scanned      {scanned} candidate file(s)")
    print(f"  not a key    {refused} of them openssl declined to read as a private key")
    for path in matches:
        facts = path.stat()
        print(f"  MATCH        {path}  {facts.st_size} bytes  mode {oct(facts.st_mode & 0o777)}")
    if not matches:
        print("  MATCH        none: no private key under these roots pairs with this certificate")
    return ZERO if matches else ONE


sys.exit(main())
