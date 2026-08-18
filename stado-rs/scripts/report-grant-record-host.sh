#!/bin/sh
# The worker-agent grant as the vault records it, beside the pre-rotation backup.
#
# After a rotation the recorded hash matched neither the delivered bearer nor any
# encoding of it. Either the mint did not store what it was handed, or something
# else re-minted the consumer afterwards. The two records answer that: a preserved
# remaining TTL is this session's rotation, a fresh TTL is a different minter.
#
# Read-only. Hash prefixes, capability counts and expiries only.
set -eu

/usr/bin/python3 - "$HOME" <<'PY'
import json
import pathlib
import sys
import time

home = pathlib.Path(sys.argv[1])
consumer = "stado-local-agent"
records = {
    "live": home / ".stado" / "skarbiec.vault.json",
    "backup_before_rotation": home / ".stado" / "skarbiec.vault.before-stado-local-agent-bearer-rotation.json",
}
now = int(time.time())
for name, path in records.items():
    if not path.is_file():
        print(f"{name}\tabsent")
        continue
    grant = (json.loads(path.read_text(encoding="utf-8")).get("tokens") or {}).get(consumer)
    if not grant:
        print(f"{name}\tno grant for {consumer}")
        continue
    print(
        f"{name}\thash={str(grant.get('hash'))[:16]}\t"
        f"caps={len(grant.get('capabilities') or [])}\t"
        f"expires_in={grant.get('expires_at', 0) - now}s\t"
        f"audience={grant.get('audience')}"
    )
PY
