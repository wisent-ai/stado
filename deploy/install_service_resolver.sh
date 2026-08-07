#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
stado_bin=${STADO_BIN:-$HOME/.stado/bin/stado}
resolver_user=${STADO_RESOLVER_USER:-$(id -un)}
if [ ! -x "$stado_bin" ]; then
  printf '%s\n' "Stado binary not found at $stado_bin" >&2
  exit 1
fi

target=${1:-}
if [ -z "$target" ]; then
  target=$($stado_bin registry self --name-only)
fi
case "$target" in
  ''|*[!a-z0-9._-]*|-*|*-) printf '%s\n' "Invalid registry target: $target" >&2; exit 1 ;;
esac

mkdir -p "$HOME/.stado/logs"
case $(uname -s) in
  Darwin)
    if [ "${STADO_RESOLVER_SYSTEM:-0}" = 1 ]; then
      rendered=$(mktemp "$HOME/.stado/stado-resolver.XXXXXX.plist")
      trap 'rm -f "$rendered"' EXIT HUP INT TERM
      sed \
        -e "s|{STADO_BIN}|$stado_bin|g" \
        -e "s|{TARGET}|$target|g" \
        -e "s|{HOME}|$HOME|g" \
        -e "s|{USER}|$resolver_user|g" \
        "$script_dir/com.wisent.stado-resolver.system.plist.tmpl" > "$rendered"
      destination=/Library/LaunchDaemons/com.wisent.stado-resolver.plist
      sudo launchctl bootout system/com.wisent.stado-resolver >/dev/null 2>&1 || true
      sudo install -o root -g wheel -m 0644 "$rendered" "$destination"
      sudo launchctl bootstrap system "$destination"
      sudo launchctl enable system/com.wisent.stado-resolver
    else
      destination=$HOME/Library/LaunchAgents/com.wisent.stado-resolver.plist
      mkdir -p "$(dirname "$destination")"
      sed \
        -e "s|{STADO_BIN}|$stado_bin|g" \
        -e "s|{TARGET}|$target|g" \
        -e "s|{HOME}|$HOME|g" \
        "$script_dir/com.wisent.stado-resolver.plist.tmpl" > "$destination"
      launchctl bootout "gui/$(id -u)/com.wisent.stado-resolver" >/dev/null 2>&1 || true
      launchctl bootstrap "gui/$(id -u)" "$destination"
      launchctl enable "gui/$(id -u)/com.wisent.stado-resolver"
      # Clean cutover: the resolver supersedes the host-pinned SSH forward.
      launchctl bootout "gui/$(id -u)/com.wisent.always-on-forward" >/dev/null 2>&1 || true
    fi
    ;;
  Linux)
    destination=$HOME/.config/systemd/user/stado-service-resolver.service
    mkdir -p "$(dirname "$destination")"
    sed \
      -e "s|{STADO_BIN}|$stado_bin|g" \
      -e "s|{TARGET}|$target|g" \
      -e "s|{HOME}|$HOME|g" \
      "$script_dir/stado-service-resolver.service.tmpl" > "$destination"
    systemctl --user daemon-reload
    systemctl --user enable --now stado-service-resolver.service
    ;;
  *)
    printf '%s\n' "Unsupported resolver host OS" >&2
    exit 1
    ;;
esac

printf '%s\n' "Installed Stado resolver for $target"
