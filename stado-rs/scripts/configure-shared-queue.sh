#!/bin/sh
set -eu

url="https://lukaszs-macbook-pro-4007-2.tail6443b3.ts.net"
namespace="probierz"
token_file="$HOME/.stado/wisent-queue-object-api-token"
config_file="${STADO_CONFIG_PATH:-$HOME/.config/stado/config.json}"

for entry in $(/bin/systemctl show wisent-agent.service --property=Environment --value 2>/dev/null || true); do
  case "$entry" in
    STADO_CONFIG_PATH=*) config_file="${entry#STADO_CONFIG_PATH=}" ;;
  esac
done

for env_file in $(/bin/systemctl show wisent-agent.service --property=EnvironmentFiles --value 2>/dev/null || true); do
  case "$env_file" in
    /*)
      [ -f "$env_file" ] || continue
      while IFS='=' read -r key value; do
        case "$key" in
          STADO_CONFIG_PATH) config_file="$value" ;;
        esac
      done <"$env_file"
      ;;
  esac
done

[ -f "$config_file" ] || {
  printf 'missing Stado config: %s\n' "$config_file" >&2
  /bin/systemctl show wisent-agent.service --property=ExecStart --property=EnvironmentFiles >&2
  exit 1
}
[ -f "$token_file" ] || {
  printf 'missing Stado queue token: %s\n' "$token_file" >&2
  exit 1
}
command -v jq >/dev/null 2>&1 || {
  printf 'jq is not installed\n' >&2
  exit 1
}
tmp="${config_file}.shared-queue.$$"
trap '/bin/rm -f "$tmp"' EXIT HUP INT TERM

/usr/bin/jq \
  --arg url "$url" \
  --arg namespace "$namespace" \
  --arg token_file "$token_file" \
  '.storage.backend = "stado"
   | .storage.stado.url = $url
   | .storage.stado.namespace = $namespace
   | .storage.stado.token_file = $token_file' \
  "$config_file" >"$tmp"
/bin/chmod --reference="$config_file" "$tmp"
/bin/chown --reference="$config_file" "$tmp"
/bin/mv "$tmp" "$config_file"
trap - EXIT HUP INT TERM
printf 'configured %s\n' "$config_file"
