#!/bin/sh
# Make the system LaunchDaemon the single, supervised Brama unit on this host.
#
# Nobody is logged in graphically here, so `gui/501` does not exist and the
# user LaunchAgent the registry declared can never be bootstrapped; every start
# fell back to an unsupervised direct process with no KeepAlive behind it. The
# LaunchDaemon runs as charles with the service env file, RunAtLoad and
# KeepAlive, which is what the other always-on units on this host already use.
#
# The file-descriptor ceiling goes up at the same time: the last outage was a
# crash loop of "accept error: Too many open files (os error 24)" against the
# 8192 soft limit.
set -eu

label=com.wisent.always-on.brama
plist=/Library/LaunchDaemons/$label.plist
sudo="/usr/bin/sudo -n"

for pattern in 'BRAMA_BIN=' 'start-with-skarbiec' 'skarbiec-entitlements-router' 'bin/brama serve'; do
    $sudo /usr/bin/pkill -TERM -f "$pattern" >/dev/null 2>&1 || true
done
/bin/sleep 2
for pattern in 'BRAMA_BIN=' 'start-with-skarbiec' 'skarbiec-entitlements-router' 'bin/brama serve'; do
    $sudo /usr/bin/pkill -KILL -f "$pattern" >/dev/null 2>&1 || true
done

if [ -f "$plist.retired" ]; then
    $sudo /bin/mv "$plist.retired" "$plist"
fi
if [ ! -f "$plist" ]; then
    printf '%s\n' "no plist to restore at $plist" >&2
    exit 1
fi

$sudo /usr/libexec/PlistBuddy -c 'Set :SoftResourceLimits:NumberOfFiles 65536' "$plist" >/dev/null 2>&1 \
    || $sudo /usr/libexec/PlistBuddy -c 'Add :SoftResourceLimits:NumberOfFiles integer 65536' "$plist" >/dev/null
$sudo /usr/libexec/PlistBuddy -c 'Set :HardResourceLimits:NumberOfFiles 65536' "$plist" >/dev/null 2>&1 \
    || $sudo /usr/libexec/PlistBuddy -c 'Add :HardResourceLimits:NumberOfFiles integer 65536' "$plist" >/dev/null
$sudo /usr/sbin/chown root:wheel "$plist"
$sudo /bin/chmod 644 "$plist"

$sudo /bin/launchctl bootout "system/$label" >/dev/null 2>&1 || true
$sudo /bin/launchctl enable "system/$label" >/dev/null 2>&1 || true
$sudo /bin/launchctl bootstrap system "$plist"
$sudo /bin/launchctl kickstart -k "system/$label" >/dev/null 2>&1 || true

n=0
while [ "$n" -lt 40 ]; do
    if /usr/bin/curl -s -o /dev/null --max-time 3 http://127.0.0.1:8080/v1/models; then
        break
    fi
    n=$((n + 1))
    /bin/sleep 1
done

printf 'listening: %s\n' "$(/usr/bin/curl -s -o /dev/null -w '%{http_code}' --max-time 5 http://127.0.0.1:8080/v1/models || echo none)"
$sudo /bin/launchctl print "system/$label" 2>/dev/null | /usr/bin/grep -E 'state = |pid = |runs = ' | head -3
/bin/rm -f "$0"
