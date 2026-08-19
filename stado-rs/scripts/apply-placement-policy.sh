#!/bin/sh
# Move the registry-published Weles placement policy into the path the worker
# reads, or refuse and change nothing.
#
# This script is embedded in the stado binary itself
# (`placement::APPLY_SCRIPT`, via include_str!). `stado host
# publish-placement-policy` delivers the document to
# $HOME/.stado/files/placement-policy.json through the audited channel and then
# runs this as one fixed remote script. It takes no arguments on purpose: a
# script that accepted a source or a destination path would be a remote writer
# with the audit trail removed. Both paths below are fixed, so the only thing
# an operator can vary is what the registry says.
#
# Three refusals, all of them silent failures somewhere else:
#
#   not JSON      a truncated or half-written delivery. Installing it takes the
#                 worker's placement loader out entirely, on every claim.
#   no _source    an unstamped document is one nobody can trace to a registry
#                 read. That is the file this whole change exists to retire: the
#                 host copy that disagreed with the registry for hours and could
#                 not be dated, attributed, or compared against it.
#   not this host a policy whose entries name no identity of this machine does
#                 not fail loudly in the worker. It resolves to `enabled: false`
#                 and the worker declines every row in silence -- 29,616 times,
#                 the last time this fleet learned it.
#
# The destination is written through a temporary file in the same directory and
# renamed, so a worker reading concurrently sees either the whole old document
# or the whole new one, never a partial write.
set -eu

src="$HOME/.stado/files/placement-policy.json"
dest_dir="$HOME/.config/weles"
dest="$dest_dir/placement-policy.json"

# Every check below is a JSON question, so a host without jq cannot answer any
# of them -- and a script that cannot verify must not write. The fleet's other
# scripts hardcode /usr/bin/jq; that path is real on the Linux hosts and absent
# on the macOS ones, where Homebrew owns it, so this one looks in the three
# places it is actually installed rather than assuming one of them.
jq=
for candidate in /usr/bin/jq /opt/homebrew/bin/jq /usr/local/bin/jq; do
  if [ -x "$candidate" ]; then
    jq=$candidate
    break
  fi
done
if [ -z "$jq" ]; then
  printf '%s\n' 'no jq on this host: refusing to install a placement policy nothing here can parse' >&2
  exit 69
fi

refuse() {
  printf 'refusing to install %s: %s\n' "$src" "$1" >&2
  exit 1
}

# The worker's own identity rule, transcribed from weles
# src/worker/identity.ts: trim, lowercase, drop trailing dots. `hostname` and
# node's `os.hostname()` are both gethostname(2), so this compares the same
# string the loader will compare.
host=$(hostname 2>/dev/null || printf '%s' '')
if [ -z "$host" ]; then
  printf '%s\n' 'this host cannot state its own hostname; the worker resolves placement by it' >&2
  exit 69
fi

# `norm` and `entry` are the loader's matching rule, applied to whichever
# document is being read: the delivery on the way in, and the file already in
# place on the way out.
filter='def norm: ascii_downcase | sub("\\.+$"; "");
def entry($h): [ .hosts[]? | select(((.hostname // "") | tostring | norm) == $h
    or (((.aliases // []) | map(tostring | norm)) | index($h) != null)) ] | .[0];'

# generation, enabled, actions -- as three tab-separated fields, for whichever
# entry belongs to this machine. `stado host publish-placement-policy` parses
# these to report the delta; an operator reading the remote output sees them
# directly.
summarize() {
  "$jq" -r --arg host "$host" "$filter"'
    ($host | norm) as $h
    | entry($h) as $e
    | [ ((._source.registry_generation // "unstamped") | tostring),
        (if $e == null then "-" else ($e.enabled | tostring) end),
        (if $e == null then "-"
         elif (($e.actions // []) | length) == 0 then "-"
         else (($e.actions | map(tostring)) | join(",")) end) ]
    | @tsv' "$1"
}

[ -f "$src" ] || refuse 'no delivered document at that path'
"$jq" -e 'type == "object"' "$src" >/dev/null 2>&1 || refuse 'it does not parse as a JSON object'
"$jq" -e '(._source | type) == "object"
  and ((._source.registry_generation // "") | tostring | length) > 0
  and ((._source.published_at // "") | tostring | length) > 0
  and ((._source.by // "") | tostring | length) > 0' "$src" >/dev/null 2>&1 \
  || refuse 'it carries no _source stamp naming the registry generation it came from'
"$jq" -e '.schema_version == 1 and (.hosts | type) == "array"' "$src" >/dev/null 2>&1 \
  || refuse 'the worker parses schema_version 1 with a hosts array, and this is not that'
"$jq" -e --arg host "$host" "$filter"'($host | norm) as $h | entry($h) != null' "$src" \
  >/dev/null 2>&1 \
  || refuse "no entry names this host ($host), so the worker would silently refuse every action"

# Read what is already there BEFORE overwriting it: after the rename nothing on
# this machine can still say what the host was running on.
if [ -L "$dest" ]; then
  printf 'refusing to write through a symlink: %s\n' "$dest" >&2
  exit 1
elif [ ! -e "$dest" ]; then
  previous=$(printf 'absent\t-\t-')
else
  previous=$(summarize "$dest" 2>/dev/null || printf '%s' '')
  [ -n "$previous" ] || previous=$(printf 'unreadable\t-\t-')
fi

/bin/mkdir -p "$dest_dir"
tmp="$dest_dir/.placement-policy.json.stado-apply.$$"
trap '/bin/rm -f "$tmp"' EXIT
/bin/cp "$src" "$tmp"
/bin/chmod 600 "$tmp"
/bin/mv "$tmp" "$dest"
trap - EXIT

installed=$(summarize "$dest")
generation=$(printf '%s' "$installed" | /usr/bin/cut -f1)

printf 'PLACEMENT_VANTAGE\t%s\n' "$host"
printf 'PLACEMENT_POLICY\tprevious\t%s\n' "$previous"
printf 'PLACEMENT_POLICY\tinstalled\t%s\n' "$installed"
printf 'installed %s at registry generation %s\n' "$dest" "$generation"
