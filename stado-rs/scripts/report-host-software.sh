#!/bin/sh
# What is this host actually running, and did Stado put it there?
#
# Every other read in this pack asks about a declaration. `service list` says a
# unit is loaded, `service show` prints the program path it always printed,
# `release status` prints the version the registry desires -- and every one of
# those answers stays true across a release that never reached the box. On
# 2026-08-18 `stado release status` printed
# `brama target=charless-mac-mini desired=0.2.27 observed=unreported` and exited
# zero, and skarbiec 0.2.1 on one machine and 0.2.3 on another -- neither of
# them in any published release -- were invisible to every screen the fleet has.
# A stale skarbiec binary stripped the `brama:agent:<id>` tags off a live
# credential every rotation for a day, and nothing anywhere could name the
# program doing it.
#
# This is the report that makes a host state what it runs. One line per program,
# carrying five things:
#
#   name        the program's basename, as an operator would say it
#   version     what the program says it is, or the literal `unknown`
#   sha256      what the bytes are
#   provenance  `release` when those exact bytes are also a staged release
#               artefact under `$HOME/.stado/releases`, `unmanaged` otherwise
#   path        where it is
#
# `provenance` is decided by digest and by nothing else. `stado host release`
# stages a release artefact under
# `$HOME/.stado/releases/<binary>/<version>/<platform>/` out of an archive whose
# SHA-256 it verified against the canonical release manifest, and then hard-links
# that staged file into place -- so a program whose digest equals a staged
# artefact's digest provably came out of a published release, and one whose
# digest equals none of them provably did not. A name, a version string and a
# timestamp are all forgeable by an `scp`; a digest that matches a verified
# archive's extracted member is not. `unmanaged` is therefore a finding and never
# a gap in this script's knowledge.
#
# Three sources make up the population, and all three are needed:
#
#   1. every program in `$HOME/.stado/bin` -- what Stado placed on this host;
#   2. every declared service unit's program -- what this host actually runs,
#      which is not the same set: a unit can name a program nothing installed;
#   3. every release-control product install path bound by the caller -- brama
#      lives at `<install_root>/bin/brama` and appears in neither of the above.
#
# Scripts are counted, not reported. The retired helper channel left 1393 shell
# scripts in `$HOME/.stado/bin` on charless-mac-mini beside 28 programs; a
# release pipeline produces none of them, so rowing each one as `unmanaged`
# would bury the twenty-eight answers the report exists to give. The shebang is the
# discriminator, spelled exactly as `host::READ_PROVENANCE_BODY` spells it,
# because two spellings of one test are two answers to "is the control-plane
# binary a helper".
#
# This script is embedded in the stado binary itself
# (`host_software::REPORT_SOFTWARE`, via include_str!) and run as one fixed
# remote script over the same audited channel `host provenance` reads with.
# Nothing is installed on the host and nothing is left behind. The caller
# prepends two bindings, both optional so the file also runs standalone:
#
#   units='<kind><TAB><unit-file path>' one per line
#   programs='<absolute program path>'   one per line
#
# Read-only, and strictly so: it hashes files, reads unit files, asks programs
# their version, and writes nothing outside its own scratch directory. It
# restarts nothing, fetches nothing, and prints no credential -- the only values
# it emits are names, versions, digests, paths and counts.
#
# Output contract, which `host_software::parse` reads:
#   * report lines on stdout, diagnostics on stderr;
#   * blank lines and lines beginning with `#` are ignored by the reader;
#   * one line per program:
#       software name=<n> version=<v|unknown> sha256=<hex|unknown>
#       provenance=<release|unmanaged> path=<absolute path>
#   * `path=` is last and carries the rest of the line verbatim, because a path
#     is the one field here that may contain a space and folding it to keep the
#     no-spaces rule would corrupt the only value an operator needs to act on;
#   * one trailer, last:
#       report reported=<count> scripts=<count>
#     a report line rather than a `#` comment, because the caller stores the
#     script count and a comment is one the reader is contracted to ignore;
#   * exit 0 whenever the reporter itself ran, including when every version is
#     unknown. Non-zero means this host cannot report at all, which the caller
#     records as an unverified report rather than as a clean one.
set -eu

PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH

# Absent when this script is run standalone, bound by the caller otherwise. An
# empty binding is not an error: the `$HOME/.stado/bin` half of the report is
# complete without either of them, and refusing to report anything because the
# caller declared no unit would make a host with no services unreportable.
: "${units:=}"
: "${programs:=}"

WORK=$(mktemp -d "${TMPDIR:-/tmp}/report-host-software.XXXXXX")
trap 'rm -rf "$WORK"' EXIT INT TERM

: >"$WORK/seen"
releases="$HOME/.stado/releases"
scripts=0
reported=0

# A file's SHA-256, or nothing when this host has no way to compute one.
# Nothing rather than a placeholder: the digest decides provenance, and a
# fabricated one would decide it wrongly.
digest_of() {
  if [ -x /usr/bin/shasum ]; then
    /usr/bin/shasum -a 256 "$1" 2>/dev/null | /usr/bin/awk '{print $1}'
  elif command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" 2>/dev/null | awk '{print $1}'
  else
    printf ''
  fi
}

# A version, or nothing, out of whatever text a program produced. Anchored on
# the semantic-version shape the release pack accepts, so a banner line
# ("stado 0.6.0"), a bare version and a `v`-prefixed tag all read the same and a
# sentence with no version in it reads as nothing. Spelled as
# `report-installed-versions.sh` spells it: two readings of "what version is
# this" that can disagree are two answers to one question.
extract_version() {
  printf '%s' "$1" |
    tr ' \t' '\n\n' |
    sed -n 's/^[vV]\{0,1\}\([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\(-[0-9A-Za-z.-]\{1,\}\)\{0,1\}\)$/\1/p' |
    head -n 1
}

# The `version` member of a one-line JSON object, or nothing. Only whitespace
# and the colon may sit between the key and its value, so `"version": null`
# reads as unparsable rather than as a version -- the same rule the release
# pack's own version reader applies.
json_version() {
  printf '%s' "$1" | tr -d '\n\r' |
    sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' |
    head -n 1
}

# A program, asked what it is. `--version` first because that is what every Rust
# binary in this fleet answers to; the `version` subcommand second because
# skarbiec prints a JSON object from one instead. stdin is closed so a program
# that would prompt cannot hold the whole report open.
program_version() {
  out=$("$1" --version 2>/dev/null </dev/null || true)
  found=$(extract_version "$out")
  if [ -z "$found" ]; then
    out=$("$1" version 2>/dev/null </dev/null || true)
    case "$out" in
      \{*) out=$(json_version "$out") ;;
    esac
    found=$(extract_version "$out")
  fi
  printf '%s' "$found"
}

# `release` when these exact bytes are also a staged release artefact, else
# `unmanaged`.
#
# Matched on the basename as well as the digest: the staging trees are the only
# place on the host where a verified published artefact is kept under its own
# coordinate, and hashing every file beneath them to answer one question would
# turn a status read into a full-tree walk of every release ever delivered.
provenance_of() {
  want_digest=$1
  want_base=$2
  if [ -z "$want_digest" ] || [ ! -d "$releases" ]; then
    printf '%s' unmanaged
    return 0
  fi
  find "$releases" -maxdepth 6 -type f -name "$want_base" >"$WORK/candidates" 2>/dev/null || :
  while IFS= read -r candidate; do
    [ -n "$candidate" ] || continue
    if [ "$(digest_of "$candidate")" = "$want_digest" ]; then
      printf '%s' release
      return 0
    fi
  done <"$WORK/candidates"
  printf '%s' unmanaged
}

# One program, reported once. A path already reported is skipped rather than
# repeated: `$HOME/.stado/bin/stado` is both an installed program and the
# program a declared unit runs, and two rows for one file would read as two
# programs disagreeing with each other.
report_program() {
  path=$1
  [ -n "$path" ] || return 0
  [ -f "$path" ] || return 0
  if grep -qxF -- "$path" "$WORK/seen" 2>/dev/null; then
    return 0
  fi
  printf '%s\n' "$path" >>"$WORK/seen"
  base="${path##*/}"
  # A `.previous` is the rollback copy of a program already reported under its
  # own name, and a dotfile is this directory's own staging litter.
  case "$base" in .*|*.previous) return 0 ;; esac
  # A compiled program is what a release pipeline produces; a shell script in
  # the same directory is what the retired helper channel left there. Counted so
  # the number is visible, not rowed so the real answers stay readable.
  #
  # Tested before the executable bit and not after, because the count has to
  # match what `host helpers` reports and that command counts a leftover by its
  # shebang alone. charless-mac-mini carries 1393 of these against 28 programs
  # and not one of them is executable any more; filtering on the exec bit first
  # made every one of them vanish from the report instead of being counted, which
  # is the accretion going quiet again in a command written to expose it.
  case "$(/usr/bin/head -c 2 "$path" 2>/dev/null)" in
    '#!')
      scripts=$((scripts + 1))
      return 0
      ;;
  esac
  # What is left has to be executable to be a program. `$HOME/.stado/bin` also
  # holds `SHA256SUMS` and a `release-manifest.json` left by earlier installs,
  # and reporting a checksum list as software this host runs would put a row in
  # front of an operator that no version and no release could ever account for.
  [ -x "$path" ] || return 0
  digest=$(digest_of "$path")
  version=$(program_version "$path")
  [ -n "$version" ] || version=unknown
  provenance=$(provenance_of "$digest" "$base")
  [ -n "$digest" ] || digest=unknown
  reported=$((reported + 1))
  printf 'software name=%s version=%s sha256=%s provenance=%s path=%s\n' \
    "$base" "$version" "$digest" "$provenance" "$path"
}

# The program one declared unit runs, read out of the unit file itself rather
# than guessed from its label: a label that merely mentions "stado" is a guess,
# and a wrong program in this report is worse than an admitted absence.
unit_program() {
  unit_kind=$1
  unit_path=$2
  case "$unit_path" in /*) ;; *) unit_path="$HOME/$unit_path" ;; esac
  [ -f "$unit_path" ] || return 0
  case "$unit_kind" in
    systemd)
      sed -n 's/^ExecStart=//p' "$unit_path" | head -n 1 | awk '{print $1}'
      return 0
      ;;
  esac
  /usr/bin/plutil -extract ProgramArguments.0 raw -o - "$unit_path" 2>/dev/null || true
}

printf '# host software report at %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"

bin="$HOME/.stado/bin"
if [ -d "$bin" ]; then
  for program in "$bin"/*; do
    report_program "$program"
  done
fi

printf '%s\n' "$units" | while IFS='	' read -r kind unit_path; do
  [ -n "$unit_path" ] || continue
  printf '%s\n' "$(unit_program "$kind" "$unit_path")"
done >"$WORK/unit-programs"
while IFS= read -r program; do
  report_program "$program"
done <"$WORK/unit-programs"

printf '%s\n' "$programs" >"$WORK/declared-programs"
while IFS= read -r program; do
  report_program "$program"
done <"$WORK/declared-programs"

# The trailer is a report line and not a comment, because the caller stores the
# script count and a `#` line is one the reader is contracted to ignore.
printf 'report reported=%s scripts=%s\n' "$reported" "$scripts"
