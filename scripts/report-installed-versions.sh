#!/bin/sh
# Report, for every binary this host has a declared `managed_versions` entry
# for, which version it is actually running.
#
# Install:
#   stado host install-helper <target> \
#     scripts/report-installed-versions.sh report-installed-versions
#
# `stado service converge` runs this over `stado host run-helper` and compares
# each `version=` against the registry's `targets[].managed_versions`. It takes
# no arguments on purpose: the fleet channel restricts helper argv to
# correlation identifiers, so everything this reads comes from the canonical
# registry this host already resolves, exactly as `probe-service-endpoints`
# does.
#
# The gap it closes: a declaration names a unit and a plist and says nothing
# about which build is behind them, so a host serving an old release is
# indistinguishable from one at the declared version. This is the installed
# half of that comparison. It is a version and not a commit because that is
# what these hosts carry: control-host runs Weles as an installed release
# artefact -- package.json, .weles-release, provenance.json, no .git anywhere --
# and asking such a tree for a commit can only ever answer "unknown" about a
# product that is in fact precisely versioned.
#
# Read-only, and strictly so: it fetches nothing, writes nothing outside its own
# scratch directory, never restarts a unit, and prints no credential -- the only
# values it emits are binary names, versions, paths, unit labels and launchd
# state. Delivery is Stado's job (`stado host release`, which
# `service converge --apply` calls), never this script's.
#
# Where an installed version comes from, in this order, first hit wins:
#
#   1. `$HOME/.stado/bin/<name>` -- an owner-only Stado program. It is asked
#      directly (`--version`, then the `version` subcommand), because a program
#      that reports its own version is the shortest true answer and both shapes
#      this fleet ships (a plain line, a JSON object) are handled.
#   2. package.json /version -- the version source a released product declares
#      for itself in `.wisent-release.json`, so this reads the same field the
#      release that produced the artefact was numbered from.
#   3. .weles-release -- the stamp the release launcher writes beside the
#      unpacked runtime, carrying the immutable coordinate the artefact was
#      fetched from.
#   4. provenance.json -- the SLSA attestation shipped inside the artefact;
#      `.version` when it carries one, otherwise the build's own tag.
#
# A product whose artefact carries none of those reports `version=unknown`.
# That is the honest answer and it is never rounded to the declared version:
# `service converge` reports it as `unknown`, never as `in-sync`.
#
# Output contract, which `stado service converge` parses:
#   * report lines on stdout, diagnostics on stderr;
#   * blank lines and lines beginning with `#` are ignored by the reader;
#   * every other line is space-separated key=value tokens, no value contains
#     a space;
#   * one line per binary this host declares a version for, emitted even when
#     nothing can be read for it:
#       binary=<name> version=<installed|unknown> root=<path> unit=<label|none>
#       state=<launchd state>
#   * `version=` is an exact semantic version or the literal `unknown` -- never
#     a partial, never empty;
#   * exit 0 whenever the helper itself ran, including when every version is
#     unknown. Non-zero means this host cannot report at all.
set -eu

PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
export PATH

STADO="$HOME/.stado/bin/stado"
[ -x "$STADO" ] || { printf '%s\n' "missing executable Stado binary: $STADO" >&2; exit 69; }

# jq is not on every managed host and a report that dies for want of it reads
# as a dead host; python3 ships with macOS and is what the beacon already uses.
PYTHON=$(command -v python3 || printf /usr/bin/python3)

WORK=$(mktemp -d "${TMPDIR:-/tmp}/report-installed-versions.XXXXXX")
trap 'rm -rf "$WORK"' EXIT INT TERM

HOST=$("$STADO" registry self --name-only 2>/dev/null | tr -d '[:space:]') || HOST=""
[ -n "$HOST" ] || { printf '%s\n' "this host is not a registry target: stado registry self failed" >&2; exit 1; }

"$STADO" registry pull >"$WORK/registry.json" 2>"$WORK/pull.err" || {
  printf 'cannot read the canonical registry: %s\n' "$(tr '\n' ' ' <"$WORK/pull.err")" >&2
  exit 1
}

# One name per line: the binaries this host declares a version for. The
# declaration is the scope -- a binary nobody declared is not this helper's
# business, and reporting it would bury the ones that are.
"$PYTHON" -c 'import json,sys
host = sys.argv[1]
doc = json.load(sys.stdin)
for target in doc.get("targets") or []:
    if target.get("name") != host:
        continue
    for name in sorted((target.get("managed_versions") or {})):
        print(name)' "$HOST" <"$WORK/registry.json" >"$WORK/binaries.txt"

# One tab-separated record per declared service of this host: label, unit-file
# path, kind. Used only to attribute a unit to an artefact, never to decide a
# version.
"$PYTHON" -c 'import json,sys
host = sys.argv[1]
doc = json.load(sys.stdin)
for target in doc.get("targets") or []:
    if target.get("name") != host:
        continue
    for service in target.get("services") or []:
        print("\t".join(str(field or "") for field in (
            service.get("label") or service.get("unit") or service.get("name"),
            service.get("path"),
            service.get("kind"),
        )))' "$HOST" <"$WORK/registry.json" >"$WORK/services.tsv"

# A version, or nothing, out of whatever text a program or a metadata file
# produced. Anchored on the semantic-version shape converge accepts, so a
# banner line ("stado 0.6.0"), a bare version and a `v`-prefixed tag all read
# the same and a sentence with no version in it reads as nothing.
extract_version() {
  printf '%s' "$1" |
    tr ' \t' '\n\n' |
    sed -n 's/^[vV]\{0,1\}\([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\(-[0-9A-Za-z.-]\{1,\}\)\{0,1\}\)$/\1/p' |
    head -n 1
}

# A JSON member, by pointer-ish path, or nothing. Missing file, unparseable
# file and absent member are one answer on purpose: each means this file did
# not tell us the version, and the caller falls through to the next source.
json_member() {
  [ -f "$1" ] || return 0
  "$PYTHON" -c 'import json,sys
try:
    doc = json.load(open(sys.argv[1]))
except Exception:
    sys.exit(0)
for key in sys.argv[2:]:
    if not isinstance(doc, dict):
        sys.exit(0)
    doc = doc.get(key)
if isinstance(doc, (str, int, float)):
    print(doc)' "$@" 2>/dev/null || true
}

# An owner-only Stado program, asked what it is. `--version` first because that
# is what every Rust binary here answers to; the `version` subcommand second
# because skarbiec prints a JSON object from one instead.
program_version() {
  out=$("$1" --version 2>/dev/null </dev/null || true)
  found=$(extract_version "$out")
  if [ -z "$found" ]; then
    out=$("$1" version 2>/dev/null </dev/null || true)
    case "$out" in
      \{*)
        printf '%s' "$out" >"$WORK/version.json"
        out=$(json_member "$WORK/version.json" version)
        ;;
    esac
    found=$(extract_version "$out")
  fi
  printf '%s' "$found"
}

# The version an installed release artefact carries about itself.
artefact_version() {
  root=$1
  found=$(extract_version "$(json_member "$root/package.json" version)")
  if [ -z "$found" ] && [ -f "$root/.weles-release" ]; then
    # `version=` when the stamp carries one, otherwise the version segment of
    # the immutable coordinate it was fetched from:
    #   release_uri=stado://releases/<product>/<version>/<platform>/<archive>
    stamped=$(sed -n 's/^version=//p' "$root/.weles-release" | head -n 1)
    [ -n "$stamped" ] || stamped=$(sed -n 's|^release_uri=stado://releases/[^/]*/\([^/]*\)/.*|\1|p' \
      "$root/.weles-release" | head -n 1)
    found=$(extract_version "$stamped")
  fi
  if [ -z "$found" ]; then
    found=$(extract_version "$(json_member "$root/provenance.json" version)")
  fi
  if [ -z "$found" ]; then
    found=$(extract_version "$(json_member "$root/provenance.json" \
      buildDefinition externalParameters tag)")
  fi
  printf '%s' "$found"
}

# Where an installed product lives, or nothing. Candidates and never a search:
# a helper that walks the filesystem looking for something called <name> finds
# a backup copy and reports its version as the running one.
artefact_root() {
  name=$1
  stem=${name%%-*}
  for candidate in \
    "$HOME/$name" \
    "$HOME/$stem" \
    "$HOME/.stado/releases/$name/current" \
    "/opt/$name"
  do
    if [ -d "$candidate" ]; then
      printf '%s' "$candidate"
      return 0
    fi
  done
  printf ''
}

# launchd state for one label, from `launchctl print` and, when the domain
# refuses it, from `launchctl list`. Spaces are folded to dashes so a state
# like `spawn scheduled` stays one token.
launchd_state() {
  state_label=$1
  state_domain=$2
  if state_out=$(/bin/launchctl print "$state_domain/$state_label" 2>/dev/null); then
    state_value=$(printf '%s\n' "$state_out" |
      awk -F'=' '$1 ~ /^[[:space:]]*state[[:space:]]*$/ {sub(/^[[:space:]]+/, "", $2); print $2; exit}')
    [ -n "$state_value" ] || state_value=loaded
  else
    state_value=$(/bin/launchctl list 2>/dev/null |
      awk -v want="$state_label" '$3 == want {print ($1 == "-" ? "loaded-not-running" : "running-pid-" $1); exit}')
    [ -n "$state_value" ] || state_value=not-loaded
  fi
  printf '%s\n' "$state_value" | tr ' ' '-'
}

systemd_state() {
  command -v systemctl >/dev/null 2>&1 || { printf 'no-systemctl\n'; return 0; }
  systemctl is-active "$1" 2>/dev/null || true
}

unit_state() {
  unit_label=$1
  unit_path=$2
  unit_kind=$3
  case "$unit_kind" in
    systemd)
      systemd_state "$unit_label"
      return 0
      ;;
  esac
  case "$unit_path" in
    /Library/LaunchDaemons/*) launchd_state "$unit_label" system ;;
    *) launchd_state "$unit_label" "gui/$(/usr/bin/id -u)" ;;
  esac
}

# The program one declared unit runs, read out of the unit file itself.
unit_program() {
  case "$3" in
    systemd)
      [ -f "$1" ] || return 0
      sed -n 's/^ExecStart=//p' "$1" | head -n 1 | awk '{print $1}'
      return 0
      ;;
  esac
  [ -f "$1" ] || return 0
  /usr/bin/plutil -extract ProgramArguments.0 raw -o - "$1" 2>/dev/null || true
}

# The declared unit whose program lives under this artefact, or nothing.
#
# Matched on the program the unit file actually names rather than on the
# binary's name: a label that merely mentions "stado" is a guess, and a wrong
# unit in a report is worse than an admitted absence.
unit_for_root() {
  match_root=$1
  [ -n "$match_root" ] || return 0
  while IFS='	' read -r u_label u_path u_kind; do
    [ -n "$u_label" ] || continue
    case "$u_path" in
      "\$HOME"/*) u_path="$HOME/${u_path#\$HOME/}" ;;
    esac
    program=$(unit_program "$u_path" "$u_label" "$u_kind")
    [ -n "$program" ] || continue
    case "$program" in
      "$match_root" | "$match_root"/*)
        printf '%s\t%s\t%s' "$u_label" "$u_path" "$u_kind"
        return 0
        ;;
    esac
  done <"$WORK/services.tsv"
  printf ''
}

printf '# host %s  registry %s  at %s\n' "$HOST" "canonical" "$(date -u +%Y-%m-%dT%H:%M:%SZ)"

count=0
while read -r binary; do
  [ -n "$binary" ] || continue
  count=$((count + 1))
  version=""
  root=""

  stado_program="$HOME/.stado/bin/$binary"
  if [ -x "$stado_program" ] && [ -f "$stado_program" ]; then
    root="$stado_program"
    version=$(program_version "$stado_program")
    [ -n "$version" ] ||
      printf '%s: %s ran but printed no version\n' "$binary" "$stado_program" >&2
  else
    root=$(artefact_root "$binary")
    if [ -n "$root" ]; then
      version=$(artefact_version "$root")
      [ -n "$version" ] ||
        printf '%s: %s carries no version metadata (package.json, .weles-release, provenance.json)\n' \
          "$binary" "$root" >&2
    else
      printf '%s: nothing installed under $HOME/%s or $HOME/.stado/bin/%s\n' \
        "$binary" "$binary" "$binary" >&2
    fi
  fi

  unit=none
  state=none
  if unit_record=$(unit_for_root "$root") && [ -n "$unit_record" ]; then
    unit=$(printf '%s' "$unit_record" | cut -f1)
    unit_path=$(printf '%s' "$unit_record" | cut -f2)
    unit_kind=$(printf '%s' "$unit_record" | cut -f3)
    state=$(unit_state "$unit" "$unit_path" "$unit_kind")
  fi

  printf 'binary=%s version=%s root=%s unit=%s state=%s\n' \
    "$binary" "${version:-unknown}" "${root:-none}" "${unit:-none}" "${state:-none}"
done <"$WORK/binaries.txt"

printf '# binaries %s\n' "$count"
