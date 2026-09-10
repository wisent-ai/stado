#!/usr/bin/env bash
# Point git and cargo at an already-installed credential file, without holding one.
#
# brama's release build fetches two private dependencies. The mac builder resolves
# them through `credential.helper store --file ~/.git-credentials-weles`; the Linux
# builder had no mechanism at all and its build died with `failed to acquire
# username/password from local configuration` after the quality gate passed.
#
# The credential itself arrives separately, as an owner-only file transferred by
# `stado host install-secret`, so no token ever appears in this file, in a command
# line, or in this host's process table. This script only wires the mechanism and
# refuses to wire it to something unsafe.
#
# Idempotent: re-running reports what is already correct and changes nothing.
set -euo pipefail

secret="${GIT_CREDENTIAL_FILE:-$HOME/.stado/git-credentials-weles}"
printf 'host %s\n' "$(hostname -s 2>/dev/null || hostname)"
printf 'secret %s\n' "$secret"

if [ ! -f "$secret" ]; then
  printf 'credential file absent; install it first with stado host install-secret\n' >&2
  exit 66
fi

# An owner-only regular file is the same bar `install-secret` enforces on the way
# in; checking it here means a later chmod cannot quietly widen it.
mode=$(/usr/bin/stat -c '%a' "$secret" 2>/dev/null || /usr/bin/stat -f '%Lp' "$secret")
if [ "$mode" != "600" ]; then
  printf 'credential file must be owner-only (600), found %s\n' "$mode" >&2
  exit 65
fi

helper="store --file $secret"
current=$(git config --global --get-all credential.helper 2>/dev/null || true)
case "$current" in
  *"$helper"*)
    printf 'credential.helper already set\n'
    ;;
  *)
    git config --global --add credential.helper "$helper"
    printf 'credential.helper added\n'
    ;;
esac

# cargo's built-in fetcher does not consult git's credential helpers on every
# platform; the CLI path does, and cargo's own error message recommends it.
cargo_config="$HOME/.cargo/config.toml"
if [ -f "$cargo_config" ] && /usr/bin/grep -q 'git-fetch-with-cli *= *true' "$cargo_config"; then
  printf 'cargo git-fetch-with-cli already true\n'
else
  mkdir -p "$(dirname "$cargo_config")"
  printf '\n[net]\ngit-fetch-with-cli = true\n' >> "$cargo_config"
  printf 'cargo git-fetch-with-cli set\n'
fi

# Prove the mechanism resolves, without printing what it resolved. `git
# credential fill` is the same lookup a fetch performs.
if printf 'protocol=https\nhost=github.com\n\n' | git credential fill >/dev/null 2>&1; then
  printf 'credential lookup for github.com succeeds\n'
else
  printf 'credential lookup for github.com FAILED\n' >&2
  exit 67
fi
