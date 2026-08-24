#!/bin/sh
# Register one delivered Skarbiec acquisition-scope catalog on this host.
#
# stado prepends exactly one line above this script when it runs it:
#   catalog_name=<shlex-quoted basename>
# naming the catalog it delivered into "$HOME/.stado/files" through the
# delivered-file channel moments earlier. Everything else about the
# registration is fixed here: the vault, the workload key, and the single
# skarbiec call. Modeled on weles's register-weles-acquisition-scopes-host.sh
# with the two appstore token re-mints removed -- minting weles worker
# credentials is not part of registering a catalog, and every re-mint
# silently extended those tokens' expiry.
set -eu
umask 077

home=${HOME:?HOME is required}
bin="$home/.stado/bin/skarbiec"
vault="$home/.stado/skarbiec.vault.json"
private_key="$home/.stado/weles-credential-workload-private.pem"
catalog="$home/.stado/files/$catalog_name"
PATH="/opt/homebrew/opt/openssl@3/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH
if [ -x /opt/homebrew/opt/openssl@3/bin/openssl ]; then
  openssl=/opt/homebrew/opt/openssl@3/bin/openssl
else
  openssl=$(command -v openssl || true)
fi

public_key=
new_private_key=
cleanup() {
  [ -z "$public_key" ] || rm -f "$public_key"
  [ -z "$new_private_key" ] || rm -f "$new_private_key"
}
trap cleanup EXIT HUP INT TERM

# The name was checked before delivery; check it again here so the file this
# script reads is decided by this script, not by whoever wrote the variable.
case "$catalog_name" in
  ""|.*|*[!A-Za-z0-9._-]*)
    printf 'invalid catalog file name\n' >&2
    exit 1
    ;;
esac

for file in "$bin" "$vault" "$private_key" "$catalog"; do
  [ -f "$file" ] || {
    printf 'required acquisition-scope file is missing: %s\n' "$file" >&2
    exit 1
  }
done
[ -n "$openssl" ] || {
  printf 'openssl is required to derive the workload public key\n' >&2
  exit 1
}

public_key=$(mktemp "$home/.stado/weles-acquisition-public.XXXXXX")

# Skarbiec accepts only an Ed25519 workload key. A host still holding an
# older key gets one Ed25519 replacement, and the new private key takes the
# canonical path only after registration with its public half succeeded.
candidate_key="$private_key"
key_description=$("$openssl" pkey -in "$private_key" -text -noout 2>/dev/null || true)
case "$key_description" in
  *ED25519*) ;;
  *)
    new_private_key=$(mktemp "$home/.stado/weles-acquisition-private.XXXXXX")
    "$openssl" genpkey -algorithm ED25519 -out "$new_private_key" >/dev/null 2>&1
    chmod 600 "$new_private_key"
    candidate_key="$new_private_key"
    ;;
esac
"$openssl" pkey -in "$candidate_key" -pubout -out "$public_key" >/dev/null 2>&1
SKARBIEC_VAULT_FILE="$vault" \
  "$bin" token-register-acquisitions "$catalog" \
    --workload-public-key-file "$public_key" \
    --replace-capabilities >/dev/null
if [ "$candidate_key" != "$private_key" ]; then
  mv -f "$candidate_key" "$private_key"
  new_private_key=
fi

printf '{"status":"reconciled","catalog":"%s"}\n' "$catalog_name"
