#!/usr/bin/env bash
# Report the launchd definition of a system daemon this host runs.
#
# `service show` prints the arguments joined into one line, which cannot
# distinguish a missing argument from an empty one, and says nothing about the
# account the job runs as. Restoring a retired unit correctly needs both, so
# this reads the plist itself.
#
# Read-only. Takes the label as its first argument.
set -u

label="${1:-com.wisent.always-on.skarbiec}"
path="/Library/LaunchDaemons/${label}.plist"

echo "path: $path"
if [ ! -e "$path" ]; then
  echo "state: absent"
else
  echo "state: present"
  echo "--- ProgramArguments ---"
  /usr/libexec/PlistBuddy -c 'Print :ProgramArguments' "$path" 2>/dev/null || echo "(none)"
  echo "--- UserName ---"
  /usr/libexec/PlistBuddy -c 'Print :UserName' "$path" 2>/dev/null || echo "(none)"
  echo "--- EnvironmentVariables ---"
  /usr/libexec/PlistBuddy -c 'Print :EnvironmentVariables' "$path" 2>/dev/null || echo "(none)"
fi

echo "--- loaded? ---"
/bin/launchctl print "system/${label}" 2>/dev/null | /usr/bin/sed -n '/state = /p;/path = /p' | head -2 || true
echo "(no lines above means launchd does not have it loaded)"
