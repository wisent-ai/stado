#!/bin/sh
# Pin the Brama LaunchDaemon to one whole release and bring it back up.
#
# The unit ran `.../brama/current/darwin-arm/bin/start-with-skarbiec`, and a
# self-hosted release runner on this host repoints `current` whenever it
# publishes. The launcher takes its GPG recipients and policy from the bundle it
# lives in while BRAMA_BIN and ENTITLEMENTS_ROUTER_BIN come from the service env
# file, so a flip of `current` silently splits one process tree across two
# releases: peer checks fail, capability redemption fails, and the alias set
# stops matching. Addressing the release directly makes the unit whole.
set -eu

label=com.wisent.always-on.brama
plist=/Library/LaunchDaemons/$label.plist
release=/Users/charles/.stado/services/brama/28b4416-cap-macos-b98f875e
launcher=$release/darwin-arm/bin/start-with-skarbiec
sudo="/usr/bin/sudo -n"

if [ ! -x "$launcher" ]; then
    printf '%s\n' "no launcher at $launcher" >&2
    exit 1
fi

$sudo /usr/libexec/PlistBuddy -c "Set :ProgramArguments:0 $launcher" "$plist"
$sudo /usr/bin/plutil -lint "$plist" >/dev/null

for pattern in 'BRAMA_BIN=' 'start-with-skarbiec' 'skarbiec-entitlements-router' 'bin/brama serve'; do
    $sudo /usr/bin/pkill -KILL -f "$pattern" >/dev/null 2>&1 || true
done

$sudo /bin/launchctl bootout "system/$label" >/dev/null 2>&1 || true
$sudo /bin/launchctl enable "system/$label" >/dev/null 2>&1 || true
$sudo /bin/launchctl bootstrap system "$plist"

n=0
while [ "$n" -lt 45 ]; do
    if /usr/bin/curl -s -o /dev/null --max-time 3 http://127.0.0.1:8080/v1/models; then
        break
    fi
    n=$((n + 1))
    /bin/sleep 1
done

printf 'program: %s\n' "$($sudo /usr/libexec/PlistBuddy -c 'Print :ProgramArguments:0' "$plist")"
printf 'listening: %s\n' "$(/usr/bin/curl -s -o /dev/null -w '%{http_code}' --max-time 5 http://127.0.0.1:8080/v1/models || echo none)"
/bin/ps ax -o pid,command \
    | /usr/bin/grep -E 'bin/brama serve|entitlements-router cap' \
    | /usr/bin/grep -v grep \
    | /usr/bin/sed 's|/Users/charles/.stado/services/brama/||'
/bin/rm -f "$0"
