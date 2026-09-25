"""Stado's host-side token custody operation; no vault or grant is ever written."""

import hashlib
import json
import os
from pathlib import Path
import stat
import sys
import tempfile
import time

# These protocol constants match Skarbiec's fixed-token validation, not deployment tuning.
TOKEN_LIMIT = 4096
OWNER_ONLY = 0o600
PRIVATE_BITS = 0o077


def token_bytes(data):
    token = data.decode("utf-8").rstrip("\r\n")
    if not token or len(token.encode()) > TOKEN_LIMIT or any(c.isspace() for c in token):
        raise ValueError("token file must contain one bounded non-whitespace token")
    return token.encode()


def token_path(value):
    home = Path.home().resolve()
    for prefix in ("~/", "$HOME/"):
        if value.startswith(prefix):
            value = str(home / value[len(prefix):])
            break
    path = Path(value)
    if not path.is_absolute():
        raise ValueError("token file must be an absolute or home-relative path")
    parent = path.parent.resolve(strict=True)
    if os.path.commonpath((str(home), str(parent))) != str(home):
        raise ValueError("token file must resolve inside the host account's home")
    return parent / path.name


def read_token(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, "rb") as source:
        metadata = os.fstat(source.fileno())
        if (not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.geteuid()
                or metadata.st_mode & PRIVATE_BITS):
            raise ValueError("token file must be an owner-controlled regular file")
        data = source.read(TOKEN_LIMIT + len(b"\r\n") + 1)
        if source.read(1):
            raise ValueError("token file exceeds the bounded token and line ending")
        return token_bytes(data)


def grant_at(vault, consumer):
    with open(vault, encoding="utf-8") as source:
        document = json.load(source)
    owner = document.get("owner")
    if not isinstance(owner, str) or not owner:
        raise ValueError("declared vault has no owner")
    grant = document.get("tokens", {}).get(consumer)
    if not isinstance(grant, dict):
        raise ValueError(f"declared vault has no grant for {consumer}")
    expiry = grant.get("expires_at")
    if type(expiry) is not int or expiry <= time.time():
        raise ValueError(f"declared grant for {consumer} is expired or has no expiry")
    if not isinstance(grant.get("capabilities"), list):
        raise ValueError(f"declared grant for {consumer} has no capability set")
    return owner, grant


def verify_token(token, grant):
    if hashlib.sha256(token).hexdigest() != grant.get("hash"):
        raise ValueError("token file does not match the declared consumer grant")


def export(vault, consumer, file):
    owner, grant = grant_at(vault, consumer)
    path = token_path(file)
    token = read_token(path)
    verify_token(token, grant)
    return {"owner": owner, "grant": grant, "token": token.decode(), "source_token_file": str(path)}


def install(vault, consumer, file, source, check, shared=False):
    # A host that reads the owner's vault through its resolver route has no
    # authoritative copy of its own; the owner's grant, verified on the owner
    # at export, is the grant its bearer must match.
    def current_grant():
        return (source["owner"], source["grant"]) if shared else grant_at(vault, consumer)

    owner, grant = current_grant()
    if owner != source["owner"] or grant != source["grant"]:
        raise ValueError("destination vault differs from source owner or consumer grant; synchronize the vault first")
    token = token_bytes(source["token"].encode())
    verify_token(token, grant)
    path = token_path(file)
    try:
        current = read_token(path)
    except FileNotFoundError:
        current = None
    if check and current != token:
        raise ValueError("destination token file is missing or does not match the declared consumer grant")
    changed = current != token
    if changed:
        staged = None
        try:
            with tempfile.NamedTemporaryFile(prefix=".stado-token-sync-", dir=path.parent, delete=False) as output:
                staged = Path(output.name)
                os.fchmod(output.fileno(), OWNER_ONLY)
                output.write(token)
                output.flush()
                os.fsync(output.fileno())
            if current_grant() != (owner, grant):
                raise ValueError("destination grant changed during token delivery")
            os.replace(staged, path)
            staged = None
        finally:
            if staged is not None:
                staged.unlink(missing_ok=True)
    verify_token(read_token(path), grant)
    if current_grant() != (owner, grant):
        raise ValueError("destination grant changed after token delivery; delivered bearer is not verified")
    return {
        "status": "token_checked" if check else "token_synced" if changed else "token_unchanged",
        "changed": changed,
        "skarbiec": {
            "ok": True,
            "consumer": consumer,
            "token_file": str(path),
            "audience": grant.get("audience"),
            "expires_at": grant["expires_at"],
            "capabilities": grant["capabilities"],
            "workload_bound": bool(grant.get("workload_public_key")),
        },
        "source_token_file": source["source_token_file"],
        "detail": "Bearer verified against the unchanged declared grant; no vault was written.",
    }


def main():
    operation, vault, consumer, file = sys.argv[1:]
    if operation == "export":
        result = export(vault, consumer, file)
    elif operation in ("install", "check", "install-shared", "check-shared"):
        result = install(
            vault,
            consumer,
            file,
            json.load(sys.stdin),
            operation.startswith("check"),
            operation.endswith("-shared"),
        )
    else:
        raise ValueError("unknown token custody operation")
    print(json.dumps(result))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise SystemExit(f"token custody refused: {error}") from None
