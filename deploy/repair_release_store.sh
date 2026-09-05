#!/usr/bin/env bash
# Repair one product's local release-catalog coordinate after an ownership fault.
#
# Release submission creates immutable revision claims and a source object before
# it mutates `stado://system/release-catalog/<product>.json`. When those creates
# persist but no durable run appears, this catalog object is the next candidate
# in source order. This helper records the real owners before deciding whether
# that diagnosis is true. It accepts only a canonical product segment, validates
# every parent component before any mutation, and changes only root-owned paths
# in this one object/metadata/lock chain. It never walks or recursively chowns the
# store.
set -euo pipefail

product="${STADO_RELEASE_STORE_PRODUCT:-}"
if ! /usr/bin/python3 - "$product" <<'PY'
import re, sys
raise SystemExit(0 if re.fullmatch(r"[A-Za-z0-9._-]+", sys.argv[1] or "") else 1)
PY
then
  printf 'invalid_product %s\n' "$product" >&2
  exit 64
fi

config="${STADO_CONFIG:-$HOME/.config/stado/config.json}"
root=$(
  /usr/bin/python3 - "$config" <<'PY'
import json, os, sys
with open(sys.argv[1], encoding="utf-8") as handle:
    data = json.load(handle)
path = ((data.get("storage") or {}).get("local") or {}).get("path") or ""
root = os.path.abspath(os.path.expanduser(path)) if path else ""
if root and os.path.realpath(root) != root:
    raise SystemExit(f"store_root has a symlinked component: {root}; nothing repaired")
print(root)
PY
)
if [ -z "$root" ] || [ ! -d "$root" ]; then
  printf 'store_root unresolved; nothing repaired\n' >&2
  exit 1
fi
case "$root" in
  "$HOME"/.stado/*) ;;
  *)
    printf 'store_root outside managed home: %s; nothing repaired\n' "$root" >&2
    exit 1
    ;;
esac

account=$(/usr/bin/id -un)
account_uid=$(/usr/bin/id -u)
group=$(/usr/bin/id -gn)
storage_key="ecosystem/system/release-catalog/$product.json"
catalog_file="$root/$storage_key"
metadata_file="$root/.metadata/$storage_key"
lock_name=$(
  /usr/bin/python3 - "$storage_key" <<'PY'
import hashlib, sys
print(hashlib.sha256(sys.argv[1].encode()).hexdigest())
PY
)
lock_file="$root/.locks/$lock_name"

printf 'release_catalog_uri stado://system/release-catalog/%s.json\n' "$product"
printf 'physical_object %s\n' "$catalog_file"
printf 'physical_metadata %s\n' "$metadata_file"
printf 'physical_lock %s\n' "$lock_file"

# First pass: establish the complete bounded source state. No ownership changes
# happen until every existing component has proven to be the expected type, not
# a symlink, and owned either by this service account or by root.
for path in \
  "$root" \
  "$root/ecosystem" \
  "$root/ecosystem/system" \
  "$root/ecosystem/system/release-catalog" \
  "$catalog_file" \
  "$root/.metadata" \
  "$root/.metadata/ecosystem" \
  "$root/.metadata/ecosystem/system" \
  "$root/.metadata/ecosystem/system/release-catalog" \
  "$metadata_file" \
  "$root/.locks" \
  "$lock_file"
do
  if [ -L "$path" ]; then
    printf 'refused_symlink %s\n' "$path" >&2
    exit 1
  fi
  if [ ! -e "$path" ]; then
    printf 'observed absent %s\n' "$path"
    continue
  fi
  case "$path" in
    "$catalog_file"|"$metadata_file"|"$lock_file")
      if [ ! -f "$path" ]; then
        printf 'refused_non_file %s\n' "$path" >&2
        exit 1
      fi
      ;;
    *)
      if [ ! -d "$path" ]; then
        printf 'refused_non_directory %s\n' "$path" >&2
        exit 1
      fi
      ;;
  esac
  owner_uid=$(/usr/bin/stat -f '%u' "$path")
  before=$(/usr/bin/stat -f '%Su:%Sg' "$path")
  if [ "$owner_uid" != "$account_uid" ] && [ "$owner_uid" != 0 ]; then
    printf 'refused_foreign_owner %s %s\n' "$before" "$path" >&2
    exit 1
  fi
  printf 'observed %s %s\n' "$before" "$path"
done

# Second pass: every possible source path is now validated. An absent component
# is left for LocalBackend to create under its now-account-owned parent.
repaired=0
for path in \
  "$root" \
  "$root/ecosystem" \
  "$root/ecosystem/system" \
  "$root/ecosystem/system/release-catalog" \
  "$catalog_file" \
  "$root/.metadata" \
  "$root/.metadata/ecosystem" \
  "$root/.metadata/ecosystem/system" \
  "$root/.metadata/ecosystem/system/release-catalog" \
  "$metadata_file" \
  "$root/.locks" \
  "$lock_file"
do
  [ ! -e "$path" ] && continue
  if [ -L "$path" ]; then
    printf 'refused_symlink %s\n' "$path" >&2
    exit 1
  fi
  owner_uid=$(/usr/bin/stat -f '%u' "$path")
  [ "$owner_uid" = "$account_uid" ] && continue
  if [ "$owner_uid" != 0 ]; then
    printf 'refused_foreign_owner uid=%s %s\n' "$owner_uid" "$path" >&2
    exit 1
  fi
  /usr/bin/sudo -n /usr/sbin/chown -h "$account:$group" "$path"
  after=$(/usr/bin/stat -f '%Su:%Sg' "$path")
  printf 'repaired %s -> %s %s\n' root "$after" "$path"
  repaired=$((repaired + 1))
done

for directory in \
  "$root" \
  "$root/ecosystem" \
  "$root/ecosystem/system" \
  "$root/ecosystem/system/release-catalog" \
  "$root/.metadata" \
  "$root/.metadata/ecosystem" \
  "$root/.metadata/ecosystem/system" \
  "$root/.metadata/ecosystem/system/release-catalog" \
  "$root/.locks"
do
  [ ! -e "$directory" ] && continue
  owner_uid=$(/usr/bin/stat -f '%u' "$directory")
  if [ "$owner_uid" != "$account_uid" ] || [ ! -w "$directory" ] || [ ! -x "$directory" ]; then
    printf 'postcondition_failed %s owner_uid=%s writable=%s searchable=%s\n' \
      "$directory" "$owner_uid" "$([ -w "$directory" ] && printf yes || printf no)" \
      "$([ -x "$directory" ] && printf yes || printf no)" >&2
    exit 1
  fi
done

printf 'release_store_repaired product=%s account=%s changed=%s bounded_paths=12\n' \
  "$product" "$account" "$repaired"
