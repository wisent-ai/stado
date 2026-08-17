#!/bin/sh
# Point the Linux workload agent at the active Stado object API.
#
# The route is fixed rather than accepted as argv: arbitrary remote fetches do
# not belong behind the fleet helper channel. Before touching systemd, use the
# agent's existing owner-only grant to prove that the active queue is readable.
# Restart only after the executable, grant and route all answer.
set -eu

unit=wisent-agent.service
route=https://charless-mac-mini.tail6443b3.ts.net
stado="$HOME/.stado/bin/stado"

[ "$(uname -s)" = Linux ] || {
    printf '%s\n' "Stado agent API configuration requires systemd" >&2
    exit 1
}
[ -x "$stado" ] || {
    printf 'missing executable Stado binary: %s\n' "$stado" >&2
    exit 1
}
/bin/systemctl is-active --quiet "$unit"

exec_start=$(/bin/systemctl show "$unit" --property=ExecStart --value)
case "$exec_start" in
    *stado*agent*) ;;
    *) printf 'refusing unexpected ExecStart: %s\n' "$exec_start" >&2; exit 1 ;;
esac

environment_files=$(/bin/systemctl show "$unit" --property=EnvironmentFiles --value)
grant_file=
for entry in $environment_files
do
    entry=${entry%%\ *}
    entry=${entry#-}
    if [ -f "$entry" ]; then
        grant_file=$entry
        break
    fi
done
[ -n "$grant_file" ] || {
    printf '%s\n' "wisent-agent.service has no readable EnvironmentFile" >&2
    exit 1
}

environment=$(/bin/systemctl show "$unit" --property=Environment --value)
/usr/bin/env -S "$environment" \
    WC_STADO_STORAGE_URL="$route" \
    STADO_API_URL="$route" \
    "$stado" storage ls queue/ >/dev/null

dropin_directory="/etc/systemd/system/${unit}.d"
dropin="$dropin_directory/stado-object-route.conf"
temporary="${dropin}.tmp.$$"
/bin/mkdir -p "$dropin_directory"
printf '[Service]\nEnvironment="WC_STADO_STORAGE_URL=%s"\nEnvironment="STADO_API_URL=%s"\n' \
    "$route" "$route" >"$temporary"
/bin/chmod 0644 "$temporary"
/bin/mv "$temporary" "$dropin"
/bin/systemctl daemon-reload
/bin/systemctl restart "$unit"
/bin/systemctl is-active --quiet "$unit"

configured=$(/bin/systemctl show "$unit" --property=Environment --value)
case " $configured " in
    *" WC_STADO_STORAGE_URL=$route "*) ;;
    *) printf '%s\n' "wisent-agent.service did not retain the canonical storage route" >&2; exit 1 ;;
esac
case " $configured " in
    *" STADO_API_URL=$route "*) ;;
    *) printf '%s\n' "wisent-agent.service did not retain the canonical API route" >&2; exit 1 ;;
esac
/usr/bin/env -S "$configured" "$stado" storage ls queue/ >/dev/null
printf '%s\n' "$route"
