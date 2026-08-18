#!/bin/sh
# Which byte string does this vault hash into a grant's `hash` field?
#
# `grant-consumer-field-read.py` compares sha256 of the trimmed bearer file with
# the recorded hash and refuses on mismatch. If the vault hashes anything else --
# the file with its trailing newline, or the raw bytes -- then that comparison
# reports every consumer as unreproducible and the refusal is about the checker,
# not about the credential. `crypto::sha256_hex` shells out to `shasum -a 256 -`,
# so the exact bytes it feeds that process are the whole question.
#
# Read-only. Hashes and a verdict only: the bearer itself is never printed, and it
# never reaches a command line -- python reads the file and hashes in-process.
set -eu

/usr/bin/python3 - "$HOME" <<'PY'
import hashlib
import json
import pathlib
import sys

home = pathlib.Path(sys.argv[1])
vault_path = pathlib.Path(
    __import__("os").environ.get("SKARBIEC_VAULT_FILE", str(home / ".stado" / "skarbiec.vault.json"))
)
consumer = "stado-local-agent"
bearer_path = home / ".stado" / "local-agent-skarbiec-token"

document = json.loads(vault_path.read_text(encoding="utf-8"))
grant = (document.get("tokens") or {}).get(consumer)
if not grant:
    print(f"no grant recorded for {consumer}")
    raise SystemExit(1)
recorded = grant.get("hash") or ""
print(f"vault_hash\t{recorded[:16]}...")
print(f"capabilities\t{len(grant.get('capabilities') or [])}")

raw = bearer_path.read_bytes()
trimmed = raw.decode().rstrip("\r\n")
candidates = {
    "trimmed": trimmed.encode(),
    "trimmed+newline": (trimmed + "\n").encode(),
    "raw_file_bytes": raw,
}
match = "none"
for name, payload in candidates.items():
    digest = hashlib.sha256(payload).hexdigest()
    mark = "MATCH" if digest == recorded else "     "
    if digest == recorded:
        match = name
    print(f"{name}\t{digest[:16]}...\t{mark}")
print(f"convention\t{match}")
PY
