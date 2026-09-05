#!/usr/bin/env bash
# Repair one release-catalog coordinate in the authority's local stores.
# Validate every primary and backup path before changing ownership; never walk
# the store or recursively chown it. A successful primary write can still fail
# when the configured backup cannot create its catalog object or metadata.
set -euo pipefail

exec /usr/bin/python3 - "${STADO_CONFIG:-$HOME/.config/stado/config.json}" "${STADO_RELEASE_STORE_PRODUCT:-}" <<'PY'
import grp
import hashlib
import json
import os
import pwd
import re
import stat
import subprocess
import sys

config, product = sys.argv[1:]
if not re.fullmatch(r"[A-Za-z0-9._-]+", product):
    raise SystemExit(f"invalid_product {product}")
with open(config, encoding="utf-8") as handle:
    storage = json.load(handle).get("storage") or {}
backup = storage.get("backup") or {}
primary_path = os.environ.get("WC_LOCAL_STORAGE_PATH") or (storage.get("local") or {}).get("path")
if not primary_path:
    raise SystemExit("store_root unresolved; nothing repaired")
paths = [primary_path]
if (os.environ.get("WC_BACKUP_STORAGE_BACKEND") or backup.get("backend")) == "local":
    backup_path = os.environ.get("WC_BACKUP_LOCAL_STORAGE_PATH") or (backup.get("local") or {}).get("path")
    if not backup_path:
        raise SystemExit("backup_store_root unresolved; nothing repaired")
    paths.append(backup_path)

uid = os.geteuid()
account = pwd.getpwuid(uid).pw_name
group = grp.getgrgid(os.getegid()).gr_name
managed_home = os.path.join(os.path.expanduser("~"), ".stado")
key = f"ecosystem/system/release-catalog/{product}.json"
lock_name = hashlib.sha256(key.encode()).hexdigest()
roots = []
nodes = {}
print(f"release_catalog_uri stado://system/release-catalog/{product}.json", flush=True)
for raw in paths:
    root = os.path.abspath(os.path.expanduser(raw))
    if root in roots:
        continue
    if os.path.commonpath([managed_home, root]) != managed_home or root == managed_home:
        raise SystemExit(f"store_root outside managed home: {root}; nothing repaired")
    if os.path.realpath(root) != root:
        raise SystemExit(f"store_root has a symlinked component: {root}; nothing repaired")
    if not os.path.isdir(root):
        raise SystemExit(f"store_root unresolved: {root}; nothing repaired")
    roots.append(root)
    files = [key, f".metadata/{key}", f".locks/{lock_name}"]
    for label, relative in zip(("physical_object", "physical_metadata", "physical_lock"), files):
        print(f"{label} {os.path.join(root, relative)}", flush=True)
        nodes[root] = True
        components = relative.split("/")
        for index in range(1, len(components) + 1):
            nodes[os.path.join(root, *components[:index])] = index < len(components)


def inspect(path, directory):
    try:
        observed = os.lstat(path)
    except FileNotFoundError:
        return None
    if stat.S_ISLNK(observed.st_mode):
        raise SystemExit(f"refused_symlink {path}")
    expected = stat.S_ISDIR if directory else stat.S_ISREG
    if not expected(observed.st_mode):
        raise SystemExit(f"refused_wrong_type {path}")
    if observed.st_uid not in (0, uid):
        raise SystemExit(f"refused_foreign_owner uid={observed.st_uid} {path}")
    return observed


# No mutation until the complete, bounded set in both stores is known.
for path, directory in nodes.items():
    observed = inspect(path, directory)
    owner = f"uid={observed.st_uid} gid={observed.st_gid}" if observed else "absent"
    print(f"observed {owner} {path}", flush=True)

repaired = 0
for path, directory in nodes.items():
    observed = inspect(path, directory)
    if observed is None or observed.st_uid == uid:
        continue
    subprocess.run(["/usr/bin/sudo", "-n", "/usr/sbin/chown", "-h", f"{account}:{group}", path], check=True)
    print(f"repaired root -> {account}:{group} {path}", flush=True)
    repaired += 1

for path, directory in nodes.items():
    observed = inspect(path, directory)
    if observed is None:
        continue
    required = os.W_OK | (os.X_OK if directory else os.R_OK)
    if observed.st_uid != uid or not os.access(path, required):
        raise SystemExit(f"postcondition_failed owner_uid={observed.st_uid} {path}")

print(f"release_store_repaired product={product} account={account} changed={repaired} stores={len(roots)} bounded_paths={len(nodes)}")
PY
