#!/usr/bin/env bash
# Print a candidate's own launch log in full, plus the environment that shaped it.
#
# The deployed launcher contains the fix and would resolve the right config when
# evaluated as the service account, yet the candidate still failed the alias check.
# Deduction has been wrong about this host five times today, so this reads the
# whole log -- it is kilobytes, not the running service's 382 MB -- together with
# the two homes the candidate could have been launched under.
#
# Read-only. Model aliases and paths are not secrets; no credential is printed.
set -euo pipefail

product="${RELEASE_PRODUCT:-brama}"
logs_root="${RELEASE_LOGS_ROOT:-$HOME/.stado/logs}"
printf 'host %s\n' "$(hostname -s 2>/dev/null || hostname)"
printf 'whoami %s home=%s\n' "$(id -un)" "$HOME"

log=$(/usr/bin/find "$logs_root" -type f -name "${product}-[0-9]*.err" 2>/dev/null \
  | /usr/bin/xargs /bin/ls -t 2>/dev/null | /usr/bin/head -1 || true)
if [ -z "$log" ]; then
  printf 'no version-named candidate log under %s\n' "$logs_root"
  exit 0
fi
printf 'log %s (%s bytes)\n' "$log" "$(/usr/bin/wc -c <"$log" | /usr/bin/tr -d ' ')"
printf -- '--- full log ---\n'
/usr/bin/cut -c1-190 "$log"

printf -- '--- candidate config candidates ---\n'
for home in "$HOME" /var/root; do
  env_file="$home/.config/brama/service.env"
  if [ -f "$env_file" ]; then
    resolved=$(/usr/bin/sed -n 's/^[[:space:]]*BRAMA_CONTROL_CONFIG[[:space:]]*=[[:space:]]*//p' "$env_file" \
      | /usr/bin/tail -1 | /usr/bin/tr -d "\"'")
    printf 'home=%s service.env present resolves=%s\n' "$home" "${resolved:-unset}"
  else
    printf 'home=%s service.env absent fallback=%s exists=%s\n' "$home" \
      "$home/.config/brama/control.json" \
      "$([ -f "$home/.config/brama/control.json" ] && echo yes || echo no)"
  fi
done
