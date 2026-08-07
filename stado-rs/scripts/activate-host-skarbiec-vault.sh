#!/bin/sh
set -eu

service_env=${BRAMA_SERVICE_ENV_FILE:-$HOME/.config/brama/service.env}
if [ -f "$service_env" ]; then
  set -a
  . "$service_env"
  set +a
fi
runtime_dir=${BRAMA_RUNTIME_DIR:-$HOME/.stado/run/brama}
version_marker=${SKARBIEC_VAULT_RELEASE_MARKER:-$HOME/.stado/backend-skarbiec-vault-version}
target_vault="$runtime_dir/vault.json"
IFS= read -r vault_version < "$version_marker"
case "$vault_version" in
  ''|*[![:alnum:]._-]*)
    printf '%s\n' 'invalid backend Skarbiec vault release marker' >/dev/stderr
    false
    ;;
esac
archive=
for candidate in "$HOME/.stado/releases/brama-vault/$vault_version"/*/brama-vault.tar.gz; do
  [ -f "$candidate" ] || continue
  if [ -n "$archive" ]; then
    printf '%s\n' 'backend Skarbiec vault release is ambiguous' >/dev/stderr
    false
  fi
  archive=$candidate
done
if [ -z "$archive" ] || [ ! -d "$runtime_dir" ]; then
  printf '%s\n' 'backend Skarbiec vault release and runtime directory are required' >/dev/stderr
  false
fi
staging=$(mktemp -d "$runtime_dir/.vault-stage.XXXXXX")
trap 'rm -rf "$staging"' EXIT HUP INT TERM
tar -xzf "$archive" -C "$staging"
source_vault="$staging/skarbiec.vault.json"
[ -f "$source_vault" ] || { printf '%s\n' 'vault release is missing skarbiec.vault.json' >/dev/stderr; false; }
chmod u=rw,go= "$source_vault"
mv "$source_vault" "$target_vault"
rm -rf "$staging"
trap - EXIT HUP INT TERM
printf '%s\n' "$target_vault"
