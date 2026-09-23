#!/bin/sh
# Bind wisent-agent.service to the GPU type declared for its registry target.
# Run through `stado host install-helper` + `run-helper`; it accepts no input.
set -eu

unit=wisent-agent.service
dropin_directory="/etc/systemd/system/${unit}.d"
dropin="$dropin_directory/gpu-type.conf"
temporary="${dropin}.tmp.$$"

/bin/systemctl is-active --quiet "$unit"
exec_start=$(/bin/systemctl show "$unit" --property=ExecStart --value)
stado_bin=$(printf '%s\n' "$exec_start" | /bin/sed -n 's/^{ path=\([^ ;}]*\).*/\1/p')
target=$(printf '%s\n' "$exec_start" | /bin/sed -n 's/.*--target[ =]\([^ ;}]*\).*/\1/p')
[ -x "$stado_bin" ] || {
  printf '%s\n' "wisent-agent.service has no executable Stado binary" >&2
  exit 1
}
[ -n "$target" ] || {
  printf '%s\n' "wisent-agent.service has no --target" >&2
  exit 1
}

environment_file=$(/bin/systemctl show "$unit" --property=EnvironmentFiles --value \
  | /usr/bin/awk '{print $1}')
[ -f "$environment_file" ] || {
  printf '%s\n' "wisent-agent.service has no readable EnvironmentFile" >&2
  exit 1
}
set -a
# shellcheck disable=SC1090
. "$environment_file"
set +a

gpu_type=$("$stado_bin" registry pull \
  | /usr/bin/python3 -c 'import json,sys
name=sys.argv[1]
target=next((item for item in json.load(sys.stdin).get("targets", []) if item.get("name") == name), None)
if not target or not target.get("gpu_type"):
    raise SystemExit(f"registry target {name!r} has no gpu_type")
print(target["gpu_type"])' "$target")

case "$gpu_type" in
  ''|*[!A-Za-z0-9._-]*)
    printf '%s\n' "registry gpu_type is not a safe CLI identifier" >&2
    exit 1
    ;;
esac

/bin/mkdir -p "$dropin_directory"
{
  printf '%s\n' '[Service]'
  printf '%s\n' 'ExecStart='
  printf 'ExecStart=%s agent --target %s --gpu-type %s\n' "$stado_bin" "$target" "$gpu_type"
} >"$temporary"
/bin/chmod 0644 "$temporary"
/bin/mv "$temporary" "$dropin"
/bin/systemctl daemon-reload
/bin/systemctl restart "$unit"
/bin/systemctl is-active --quiet "$unit"
/bin/systemctl show "$unit" --property=ExecStart --value
/bin/journalctl -u "$unit" --no-pager --all --output=cat -n 20 \
  --grep 'claim|queue|inference|yieldable|pause|error|failed'
