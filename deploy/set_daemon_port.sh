#!/usr/bin/env bash
# Give the Skarbiec daemon the port its own fleet declaration promises.
#
# The unit ran `skarbiec serve` with no port, so it bound the binary's default
# while three separate records -- the placement profile's health probe, the
# service directory endpoint, and the ssh permitopen list -- all named a
# different one. Nothing was ever down; the port was simply never passed, and a
# hand-started process had been covering the difference until it went away.
#
# The port is read from the placement profile rather than written here, so the
# unit and the probe that judges it can no longer disagree. The lookup is
# anchored on the unit label this script edits, so it does not depend on what
# the host calls itself.
set -eu

label="com.wisent.always-on.skarbiec"
path="/Library/LaunchDaemons/${label}.plist"
plistbuddy=/usr/libexec/PlistBuddy
stado="$HOME/.stado/bin/stado"

[ -e "$path" ] || { echo "no daemon at $path" >&2; exit 1; }
[ -x "$stado" ] || { echo "no stado at $stado to read the declaration with" >&2; exit 1; }

port=$("$stado" registry pull 2>/dev/null | /usr/bin/awk -v label="$label" '
  /"service": "skarbiec"/ { want = 1; next }
  want && match($0, /127\.0\.0\.1:[[:digit:]]+/) {
    found = substr($0, RSTART, RLENGTH); sub(/.*:/, "", found); candidate = found; want = 0; next
  }
  index($0, "\"name\": \"" label "\"") { print candidate; exit }
')

case "$port" in
  "" | *[!0-9]* ) echo "the profile declares no health probe port for $label" >&2; exit 1 ;;
esac
echo "declared port: $port"

sudo=""
if [ "$(/usr/bin/id -u)" != "0" ]; then sudo="/usr/bin/sudo -n"; fi

current=$($plistbuddy -c 'Print :ProgramArguments' "$path" 2>/dev/null | /usr/bin/tr -d ' ' | /usr/bin/tr '\n' ' ')
case "$current" in
  *--port*) echo "already carries a port: $current" ;;
  *)
    $sudo $plistbuddy -c "Add :ProgramArguments: string --port" "$path"
    $sudo $plistbuddy -c "Add :ProgramArguments: string ${port}" "$path"
    echo "added --port ${port}"
    ;;
esac

echo "--- arguments now ---"
$plistbuddy -c 'Print :ProgramArguments' "$path"

$sudo /bin/launchctl bootout "system/${label}" 2>/dev/null || true
$sudo /bin/launchctl bootstrap system "$path"
echo "--- state ---"
/bin/launchctl print "system/${label}" 2>/dev/null | /usr/bin/sed -n '/state = /p' | head -1
