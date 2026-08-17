#!/usr/bin/env bash
# Every storage path this host's Stado configuration names, and what that path
# actually is on disk.
#
# `JobStorage::new()` builds the primary backend, then the configured read
# failover, and a `LocalBackend` root goes through `fs::create_dir_all`. mkdir
# answers EEXIST for a path that exists as a dangling symlink or as a file, so a
# root left pointing at a removed disk turns into `agent loop failed: File
# exists (os error 17)` -- which is what the only GPU host in the registry has
# been printing every ten seconds while `systemctl is-active` said `active`.
#
# Read-only. Token values are never printed: only whether the file exists.
set -euo pipefail

config=${STADO_CONFIG:-/root/.config/stado/config.json}
printf 'CONFIG\t%s\n' "$config"
[ -r "$config" ] || { printf 'ERROR\tunreadable\n' >&2; exit 1; }

if ! command -v jq >/dev/null 2>&1; then
  printf 'ERROR\tjq unavailable; cannot walk the document safely\n' >&2
  exit 1
fi

printf '\nSTORAGE_KEYS\n'
jq -r '
  def walk_paths($prefix):
    to_entries[] |
    if (.value | type) == "object"
    then .value | walk_paths($prefix + .key + ".")
    else "\($prefix)\(.key)\t\(.value | if type == "string" then . else tostring end)"
    end;
  .storage // {} | walk_paths("storage.")
' "$config" | while IFS=$'\t' read -r key value; do
  case "$key" in
    *token*|*secret*|*password*) printf '%s\t(value withheld)\n' "$key" ;;
    *) printf '%s\t%s\n' "$key" "$value" ;;
  esac
done

printf '\nPATH_STATE\n'
jq -r '
  def walk_paths($prefix):
    to_entries[] |
    if (.value | type) == "object"
    then .value | walk_paths($prefix + .key + ".")
    else "\($prefix)\(.key)\t\(.value | if type == "string" then . else tostring end)"
    end;
  .storage // {} | walk_paths("storage.")
' "$config" | while IFS=$'\t' read -r key value; do
  case "$value" in
    /*) ;;
    *) continue ;;
  esac
  if [ -L "$value" ]; then
    target=$(readlink "$value")
    if [ -e "$value" ]; then
      printf '%s\t%s\tsymlink -> %s (resolves)\n' "$key" "$value" "$target"
    else
      printf '%s\t%s\tDANGLING SYMLINK -> %s\n' "$key" "$value" "$target"
    fi
  elif [ -d "$value" ]; then
    printf '%s\t%s\tdirectory\n' "$key" "$value"
  elif [ -f "$value" ]; then
    printf '%s\t%s\tfile\n' "$key" "$value"
  elif [ -e "$value" ]; then
    printf '%s\t%s\texists (other type)\n' "$key" "$value"
  else
    printf '%s\t%s\tabsent\n' "$key" "$value"
  fi
done

printf '\nSTADO_DIR\n'
ls -la /root/.stado 2>&1 | head -n 40 || true
