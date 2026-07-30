#!/bin/sh
# Remove Stado services and release binaries. Durable queue/config data is
# preserved unless --purge-data is supplied explicitly.
set -eu

purge=false
case "${1:-}" in
  "") ;;
  --purge-data) purge=true ;;
  *) printf 'usage: %s [--purge-data]\n' "$0"; false ;;
esac

if [ "${STADO_UNINSTALL_CONFIRM:-}" != "uninstall-stado" ]; then
  printf 'Refusing destructive operation. Re-run with STADO_UNINSTALL_CONFIRM=uninstall-stado.\n'
  false
fi

home=${HOME:?HOME is required}

if [ "$(uname -s)" = Darwin ]; then
  domain="gui/$(id -u)"
  for plist in "$home"/Library/LaunchAgents/com.wisent.compute.*.plist; do
    [ -e "$plist" ] || continue
    launchctl bootout "$domain" "$plist" >/dev/null || true
    rm -f "$plist"
  done
else
  unit_dir="$home/.config/systemd/user"
  for unit in "$unit_dir"/com.wisent.compute.*.service; do
    [ -e "$unit" ] || continue
    systemctl --user disable --now "$(basename "$unit")" >/dev/null || true
    rm -f "$unit"
  done
  systemctl --user daemon-reload >/dev/null || true
fi

for name in stado wc stado-coverage stado-fix stado-watchdog stado-mcp; do
  rm -f "$home/.stado/bin/$name" "$home/.stado/bin/$name.previous"
  if [ -L "$home/.local/bin/$name" ]; then
    rm -f "$home/.local/bin/$name"
  fi
done
rm -f "$home/.stado/bin/release-manifest.json" "$home/.stado/bin/SHA256SUMS"

if [ "$purge" = true ]; then
  rm -rf "$home/.stado/local-storage" "$home/.stado/local-backup"
  rm -f "$home/.stado/config.json"
  printf 'Stado services, binaries, local queue data, and config removed.\n'
else
  printf 'Stado services and binaries removed. Config and queue data were preserved.\n'
fi
