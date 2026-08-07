#!/bin/sh
# Why did a KeepAlive daemon exit? Newest macOS crash report headers for one
# process name, or silence if it exited without crashing.
#
# `host run-helper` passes no arguments on purpose, so this asks about the
# daemon whose restarts the placement row surfaced.
set -eu

name=${CRASH_PROCESS_NAME:-brama}
reports=$HOME/Library/Logs/DiagnosticReports

/usr/bin/find "$reports" -name "$name*" -newermt '-1 day' -print |
    while read -r file; do
        printf '== %s ==\n' "$file"
        /usr/bin/grep -E 'Exception Type|Termination|Exit Reason' "$file" || printf 'no reason header\n'
    done
printf 'end of reports\n'
