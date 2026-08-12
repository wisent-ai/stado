#!/bin/sh
# Every helper script sitting in this host's ~/.stado/bin, with its age and its
# size. The inventory nothing produced while the directory filled up.
#
# `stado host install-helper` writes into that directory and nothing removes
# what it wrote. control-host carries 553 installed helper scripts beside
# 16 binaries: each one was delivered to settle one incident, none was ever
# withdrawn, and the only record of what any of them was for is an operator's
# memory. A count is what makes that visible; `stado host provenance` already
# prints it as a footnote, and this is the inventory behind the footnote.
#
# Install:
#   stado host install-helper <target> \
#     stado-rs/scripts/report-stale-helpers.sh report-stale-helpers
#
# `stado host helpers <target>` runs this and renders it oldest first. It takes
# no arguments on purpose, exactly as probe-service-endpoints.sh does: the
# fleet channel restricts helper argv to correlation identifiers, and a helper
# that accepted a path or an age threshold would be a remote file walker, or a
# remote reaper, with the audit trail removed. The threshold is applied by the
# caller against these numbers, and every removal goes back over the audited
# channel one named helper at a time.
#
# Read-only throughout: it stats files and writes nothing anywhere. Deciding
# what to delete is not a decision a script installed on 553 hosts gets to make.
#
# One line per helper, tab-delimited:
#
#   <name>\t<mtime seconds since epoch>\t<size in bytes>
#
# A raw epoch rather than a rendered date, because the reader is comparing it
# against a threshold and a localised date string would have to be parsed back.
set -eu

bin="$HOME/.stado/bin"
[ -d "$bin" ] || exit 0

# BSD and GNU stat share no format flag. Resolved once rather than per file:
# probing inside the loop would run an extra process for each of 553 entries.
# Space-separated and split below rather than asking stat for the tab: a
# literal tab inside a quoted format string is invisible to the next reader of
# this file, and this output is parsed by a caller that will not forgive it.
if [ "$(/usr/bin/uname -s)" = "Darwin" ]; then
  stat_format='-f%m %z'
else
  stat_format='-c%Y %s'
fi

for program in "$bin"/*; do
  [ -f "$program" ] || continue
  name="${program##*/}"
  # Dotfiles are this directory's own staging litter (`.<name>.stado-install.$$`
  # and friends), and a `.previous` is the rollback copy of a program already
  # listed under its own name. Neither is a helper somebody installed.
  case "$name" in .*|*.previous) continue ;; esac
  # The shebang is the honest discriminator between a delivered script and a
  # compiled release artifact, and it is readable without executing anything.
  # Spelled exactly as `host::READ_PROVENANCE_BODY` spells it, because two
  # spellings of one test are two answers to "is the control-plane binary a
  # helper" -- and the wrong one puts `stado` itself on a reaping list.
  case "$(/usr/bin/head -c 2 "$program" 2>/dev/null)" in '#!') ;; *) continue ;; esac
  # A file that cannot be stat'd is skipped rather than reported with a made-up
  # age: a fabricated zero would sort to the top of an oldest-first list and be
  # the first thing a `--prune` run removed.
  facts=$(/usr/bin/stat "$stat_format" "$program" 2>/dev/null) || continue
  # Neither half is usable alone: an entry with one field is a stat this script
  # did not understand, and printing it would hand the caller a short row.
  set -- $facts
  [ "$#" -eq 2 ] || continue
  printf '%s\t%s\t%s\n' "$name" "$1" "$2"
done
