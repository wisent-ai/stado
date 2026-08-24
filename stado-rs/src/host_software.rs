//! What is this host actually running, and did Stado put it there?
//!
//! On 2026-08-18 `stado release status` printed
//! `brama target=control-host desired=0.2.27 observed=unreported` and
//! exited zero. A host that had never once said what it runs was rendered
//! indistinguishable from a healthy one, in the command an operator reaches for
//! to ask exactly that. On the same day two machines were running a skarbiec
//! built on somebody's laptop — 0.2.1 here, 0.2.3 on control-host, neither
//! of them in any published release — and the pre-fix binary was stripping the
//! `brama:agent:<id>` tags off a live credential every rotation, which removed a
//! working subscription from the fleet while the credential itself stayed valid.
//! No screen in this fleet could name the program doing it, because no screen
//! knew the program existed.
//!
//! Three separate silences, one shape: the fleet stored **declarations** about
//! software and never an **observation** of it. `managed_versions` says what a
//! host must run. `release_control.desired` says what must be rolled out. A
//! service declaration names a unit and a plist. All three stay true across
//! every release that never reached the box, and none of them is about the bytes
//! on the disk.
//!
//! So this module records the other half, and records it the way
//! [`crate::observations`] records everything else that decays: as a look, taken
//! at a moment, by a named vantage, that goes stale. One row per program:
//! `{ name, path, version, sha256, provenance }`.
//!
//! `provenance` is [`RELEASE`] when those exact bytes are also a staged release
//! artefact under `$HOME/.stado/releases`, and [`UNMANAGED`] otherwise. It is
//! decided by digest and by nothing else, on the host, because a name, a version
//! string and a program's own claim about its provenance all survive one `scp`,
//! and a digest that equals the extracted member of an archive Stado verified
//! against the canonical release manifest does not.
//! [`crate::deploy::host_release`] stages every delivery under its own immutable
//! coordinate and hard-links it into place, so a program that came through the
//! sanctioned channel matches and one that did not, does not — which makes
//! `unmanaged` a finding rather than a gap in what this could measure.
//!
//! **Silence is a failure here, and that is the whole point.** A host with no
//! report, a report older than [`crate::observations::DEFAULT_TTL`], a declared
//! program that is absent, an `unmanaged` program, or a version that disagrees
//! with what the fleet declares are all failures out of [`judge`], each in one
//! sentence that names the host and the exact disagreement.
//!
//! What is deliberately *not* a failure is a program nothing declares. This
//! laptop carries eleven dated backup copies of `stado` in `$HOME/.stado/bin`,
//! none of them running, and failing forever on those is how an operator learns
//! to write `|| true` after the command — at which point the drift this exists
//! to catch stops being noticed again, exactly as
//! `service_converge::report_gate` argues. Every such program is still reported,
//! still counted and still visible in `stado host software`; it just does not
//! decide the gate. Accountability is resolved against the live registry on
//! every read rather than frozen into the record, for the reason
//! [`crate::provenance`] does not store reachability: a declaration added an
//! hour after a report must bring that program into scope, and a stored verdict
//! would still be answering the older question.

use std::collections::{BTreeMap, BTreeSet};
use std::io;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::deploy::service;
use crate::deploy::{host_channel, shlex_quote, DeployError, Runner};
use crate::observations::{self, Freshness, Observation, OBSERVED, UNVERIFIED};
use crate::targets::ComputeTarget;

/// The bytes came out of a release Stado published and verified.
pub const RELEASE: &str = "release";
/// The bytes match no release artefact this host carries. A finding.
pub const UNMANAGED: &str = "unmanaged";
/// The reporter looked and the program would not say. Never rounded to a
/// version, and never rounded to agreement.
pub const UNKNOWN: &str = "unknown";
/// What [`report_fact`] prefixes, named once because [`reported_hosts`] reads it
/// back off the fact.
const REPORT_KIND: &str = "software-report:";

/// The reporter that reads every program on the host and states what it is,
/// embedded in this binary and run as one fixed remote script.
///
/// Kept as a checked-in file rather than a string literal so it is reviewed and
/// read as the shell program it is, exactly as `service_converge::VERSION_PROBE`
/// is. Nothing is installed on the host: the helper channel that used to put
/// scripts there was removed for putting unreviewed ones there, and this travels
/// inside the binary instead.
const REPORT_SOFTWARE: &str = r#"#!/bin/sh
# What is this host actually running, and did Stado put it there?
#
# Every other read in this pack asks about a declaration. `service list` says a
# unit is loaded, `service show` prints the program path it always printed,
# `release status` prints the version the registry desires -- and every one of
# those answers stays true across a release that never reached the box. On
# 2026-08-18 `stado release status` printed
# `brama target=control-host desired=0.2.27 observed=unreported` and exited
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
# scripts in `$HOME/.stado/bin` on control-host beside 28 programs; a
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
  # shebang alone. control-host carries 1393 of these against 28 programs
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
"#;

/// The canonical fact name for "what is this program on this host".
///
/// One spelling, shared by the writer and by every reader, for the reason
/// [`crate::observations::service_fact`] has one: a fact recorded under one name
/// and looked up under another is a fact with no reader.
pub fn software_fact(name: &str, host: &str) -> String {
    format!("software:{name}@{host}")
}

/// The canonical fact name for "did this host report its software at all".
///
/// A separate fact from the programs it lists, and the one that makes silence
/// legible: per-program rows can only ever say what was there, so without a row
/// for the report itself a host that never answered and a host whose programs
/// were all removed would read identically. It also bounds the report —
/// [`observations::record`] merges and never deletes, so a program gone from the
/// host would otherwise stay on file forever and read as present.
pub fn report_fact(host: &str) -> String {
    format!("{REPORT_KIND}{host}")
}

/// One program on one host, as that host reported it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostSoftware {
    /// The program's basename, as an operator would say it.
    pub name: String,
    /// Where it is on the host. Absolute, and the one field here that may
    /// contain a space.
    pub path: String,
    /// What the program says it is, or [`UNKNOWN`].
    pub version: String,
    /// What the bytes are, lowercase hex, or [`UNKNOWN`] when the host has no
    /// way to compute one.
    pub sha256: String,
    /// [`RELEASE`] or [`UNMANAGED`], as the host's own digest comparison
    /// decided. A word from a newer reporter is carried through verbatim rather
    /// than rounded to whichever of these two it resembles.
    pub provenance: String,
}

impl HostSoftware {
    pub fn is_release(&self) -> bool {
        self.provenance == RELEASE
    }

    /// The four fields the fact name does not carry, in the shape
    /// [`Observation`]'s detail keeps them.
    ///
    /// `path` last and unquoted, for the reason the wire format puts it last: it
    /// is the only value that may contain a space, and every reader takes the
    /// rest of the line for it.
    fn detail(&self) -> String {
        format!(
            "version={} sha256={} provenance={} path={}",
            self.version, self.sha256, self.provenance, self.path
        )
    }

    /// The inverse of [`Self::detail`], against a name taken from the fact.
    ///
    /// `None` for anything that is not a whole row. A missing path or a missing
    /// provenance is not a row with a default; completing it would put a
    /// fabricated `unmanaged` in front of an operator about bytes nothing read.
    fn from_detail(name: &str, detail: &str) -> Option<Self> {
        let (head, path) = match detail.split_once("path=") {
            Some((head, path)) => (head, path.trim()),
            None => (detail, ""),
        };
        let mut row = Self {
            name: name.to_string(),
            path: path.to_string(),
            version: UNKNOWN.to_string(),
            sha256: UNKNOWN.to_string(),
            provenance: String::new(),
        };
        for token in head.split_whitespace() {
            if let Some(value) = token.strip_prefix("version=") {
                row.version = value.to_string();
            } else if let Some(value) = token.strip_prefix("sha256=") {
                row.sha256 = value.to_string();
            } else if let Some(value) = token.strip_prefix("provenance=") {
                row.provenance = value.to_string();
            }
        }
        if row.path.is_empty() || row.provenance.is_empty() {
            return None;
        }
        Some(row)
    }

    pub fn json(&self) -> Value {
        json!({
            "name": self.name,
            "path": self.path,
            "version": self.version,
            "sha256": self.sha256,
            "provenance": self.provenance,
        })
    }

    /// The digest, short enough to read inside a sentence and long enough to
    /// look up. A full 64 characters mid-sentence is a sentence nobody reads.
    fn short_digest(&self) -> &str {
        self.sha256.get(..12).unwrap_or(&self.sha256)
    }
}

/// The newest software report one host has on file, and how old it is.
#[derive(Debug, Clone)]
pub struct Report {
    pub host: String,
    /// One row per program the newest report listed.
    pub rows: Vec<HostSoftware>,
    /// Shell scripts the host carries alongside. Counted rather than rowed: the
    /// retired helper channel left 1393 of them in `$HOME/.stado/bin` on
    /// control-host against 28 programs, and a release pipeline produces
    /// none of them — rowing each as `unmanaged` would bury the twenty-eight
    /// answers the report exists to give.
    pub scripts: usize,
    /// How old the fleet's knowledge of this host's software is.
    /// [`Freshness::Never`] is the state that was invisible.
    pub freshness: Freshness,
}

impl Report {
    /// Nothing on file for this host. Kept apart from an empty report: a host
    /// that carries no programs answered, and one that never answered did not.
    pub fn never(host: &str) -> Self {
        Self {
            host: host.to_string(),
            rows: Vec::new(),
            scripts: usize::default(),
            freshness: Freshness::Never,
        }
    }

    /// The state word of the look itself: [`OBSERVED`] when the host answered,
    /// [`UNVERIFIED`] when the look could not happen, `never` when none was ever
    /// taken, or a word from a newer writer carried through.
    pub fn state(&self) -> &str {
        match &self.freshness {
            Freshness::Fresh(row) | Freshness::Stale(row) => row.state.as_str(),
            Freshness::Never => "never",
        }
    }

    /// Why, in the reporter's or the channel's own words. Empty when the host
    /// answered cleanly.
    pub fn refusal(&self) -> &str {
        match &self.freshness {
            Freshness::Fresh(row) | Freshness::Stale(row) if row.state != OBSERVED => {
                row.detail.as_str()
            }
            _ => "",
        }
    }

    /// `just now`, `14m ago`, `stale (3h)` or `never`, in the one spelling every
    /// other freshness column in this tree uses.
    pub fn age(&self) -> String {
        observations::render(&self.freshness)
    }

    pub fn released(&self) -> usize {
        self.rows.iter().filter(|row| row.is_release()).count()
    }

    pub fn unmanaged(&self) -> usize {
        self.rows.iter().filter(|row| !row.is_release()).count()
    }

    pub fn find(&self, name: &str) -> Option<&HostSoftware> {
        self.rows.iter().find(|row| row.name == name)
    }

    /// The counts as one phrase, for a row that has one column to say them in.
    pub fn summary(&self) -> String {
        if matches!(self.freshness, Freshness::Never) {
            return "no report".to_string();
        }
        format!(
            "{} program(s), {} release, {} unmanaged, {} script(s)",
            self.rows.len(),
            self.released(),
            self.unmanaged(),
            self.scripts
        )
    }

    pub fn json(&self) -> Value {
        json!({
            "host": self.host,
            "state": self.state(),
            "observed": self.age(),
            "detail": self.refusal(),
            "reported": self.rows.len(),
            "release": self.released(),
            "unmanaged": self.unmanaged(),
            "scripts": self.scripts,
            "programs": self.rows.iter().map(HostSoftware::json).collect::<Vec<Value>>(),
        })
    }
}

// ---------------------------------------------------------------------------
// Reading the host
// ---------------------------------------------------------------------------

/// [`REPORT_SOFTWARE`] with the caller's declarations bound ahead of it.
///
/// The unit files come from the registry and the extra programs from the
/// release-control policy, because both are declarations and declarations live on
/// the control plane. The host is asked to read files and hash bytes; it is never
/// asked which of its files matter, which is how a reporter ends up carrying an
/// opinion the registry never authorized.
fn reporter(units: &[(String, String)], programs: &[String]) -> String {
    let units: Vec<String> = units
        .iter()
        .map(|(kind, path)| format!("{kind}\t{path}"))
        .collect();
    format!(
        "units={}\nprograms={}\n{REPORT_SOFTWARE}",
        shlex_quote(&units.join("\n")),
        shlex_quote(&programs.join("\n"))
    )
}

/// The unit files TARGET declares, as `(kind, path)` pairs for the reporter.
fn declared_units(target: &ComputeTarget) -> Vec<(String, String)> {
    service::declared_services(target)
        .into_iter()
        .filter(|declared| !declared.path.is_empty())
        .map(|declared| (declared.kind, declared.path))
        .collect()
}

/// Ask TARGET what it runs.
///
/// One round trip on the same audited channel `host provenance` reads with, and
/// nothing is installed on the host: the reporter travels with stado, so a
/// failure here is the remote's own words about this read and never a remedy for
/// a delivery channel that no longer exists. Reading what is installed is a
/// status read, so it runs under the channel's ordinary read bound.
pub async fn gather(
    target: &ComputeTarget,
    programs: &[String],
    runner: &Runner,
) -> Result<(Vec<HostSoftware>, usize), DeployError> {
    let script = reporter(&declared_units(target), programs);
    let output = host_channel::run_script_with_timeout(
        target,
        &script,
        host_channel::remote_timeout(),
        runner,
    )
    .await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the software reporter did not complete",
        )));
    }
    Ok(parse(&output.stdout))
}

/// The reporter's stdout, as one row per program plus the script count.
///
/// Line-oriented `key=value` rather than JSON for the reason
/// `service_converge::parse_report` gives: a shell script that has to emit valid
/// JSON emits invalid JSON the first time a path contains a quote. Blank lines
/// and `#` comments are skipped and unknown keys are ignored, so the reporter can
/// add a field without a matching release here.
pub fn parse(stdout: &str) -> (Vec<HostSoftware>, usize) {
    let mut rows: Vec<HostSoftware> = Vec::new();
    let mut scripts = usize::default();
    for line in stdout.lines() {
        let line = line.trim();
        if let Some(body) = line.strip_prefix("software ") {
            // The wire order is the storage order, so one decoder serves both
            // and they cannot disagree about where `path=` begins. Only the name
            // is lifted out, because it is the half the fact name carries.
            let Some((_, rest)) = body.split_once("name=") else {
                continue;
            };
            let (name, rest) = rest.split_once(' ').unwrap_or((rest, ""));
            if name.is_empty() {
                continue;
            }
            if let Some(row) = HostSoftware::from_detail(name, rest) {
                rows.push(row);
            }
        } else if let Some(body) = line.strip_prefix("report ") {
            for token in body.split_whitespace() {
                if let Some(value) = token.strip_prefix("scripts=") {
                    scripts = value.parse().unwrap_or_default();
                }
            }
        }
    }
    (rows, scripts)
}

// ---------------------------------------------------------------------------
// Keeping the newest report
// ---------------------------------------------------------------------------

/// The roster row's detail: what the newest report listed, so a later read can
/// tell the report apart from every row ever written for this host.
fn roster_detail(names: &[&str], scripts: usize) -> String {
    format!(
        "reported={} scripts={scripts} names={}",
        names.len(),
        names.join(",")
    )
}

/// Persist one host's report, replacing whatever was on file for it.
///
/// Written through [`observations::record`], the file that already answers "when
/// did anyone last look" for this fleet, because a second store for the same kind
/// of fact is a second answer to that question. The roster row is what makes
/// replacement expressible in a store that merges and never deletes: a program
/// dropped from the newest report is dropped from the roster, so it stops being
/// part of the report even though its own row is still on file.
///
/// The vantage is the target, not this machine. The look happened on that host —
/// its files, its digests, its programs — and recording an operator's laptop as
/// the vantage would let two operators' runs overwrite each other's evidence
/// about a third machine.
pub fn record(host: &str, rows: &[HostSoftware], scripts: usize) -> io::Result<()> {
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    let mut written: Vec<Observation> = rows
        .iter()
        .map(|row| Observation::now(software_fact(&row.name, host), host, OBSERVED, row.detail()))
        .collect();
    written.push(Observation::now(
        report_fact(host),
        host,
        OBSERVED,
        roster_detail(&names, scripts),
    ));
    observations::record(&written)
}

/// Record that the look could not happen, in the channel's own words.
///
/// A failed read is written and not swallowed, because the alternative leaves the
/// previous report on file looking current — the exact shape of the twelve-day
/// outage [`crate::observations`] was built against. The roster keeps the names it
/// had, so the last thing anyone saw is still readable and is now visibly
/// unverified.
pub fn record_refusal(host: &str, detail: &str) -> io::Result<()> {
    let held = load(host);
    let names: Vec<&str> = held.rows.iter().map(|row| row.name.as_str()).collect();
    observations::record(&[Observation::now(
        report_fact(host),
        host,
        UNVERIFIED,
        format!("{} {detail}", roster_detail(&names, held.scripts)),
    )])
}

/// The roster row for one host: its freshness, the program names it listed, and
/// the script count beside them.
fn roster(records: &[Observation], host: &str) -> Option<(Freshness, BTreeSet<String>, usize)> {
    let fact = report_fact(host);
    let freshness = observations::freshness_in(records, &fact, observations::DEFAULT_TTL);
    let row = match &freshness {
        Freshness::Fresh(row) | Freshness::Stale(row) => row.clone(),
        Freshness::Never => return None,
    };
    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut scripts = usize::default();
    for token in row.detail.split_whitespace() {
        if let Some(value) = token.strip_prefix("names=") {
            names.extend(
                value
                    .split(',')
                    .filter(|name| !name.is_empty())
                    .map(str::to_string),
            );
        } else if let Some(value) = token.strip_prefix("scripts=") {
            scripts = value.parse().unwrap_or_default();
        }
    }
    Some((freshness, names, scripts))
}

/// The newest report on file for one host.
pub fn load(host: &str) -> Report {
    load_in(&observations::load(), host)
}

/// [`load`] against records already in hand, for a reader asking about every
/// target in one rendering — the same reason [`observations::describe_in`]
/// exists.
pub fn load_in(records: &[Observation], host: &str) -> Report {
    let Some((freshness, names, scripts)) = roster(records, host) else {
        return Report::never(host);
    };
    let rows: Vec<HostSoftware> = names
        .iter()
        .filter_map(|name| {
            let fact = software_fact(name, host);
            records
                .iter()
                .filter(|row| row.fact == fact)
                .max_by(|left, right| left.at.cmp(&right.at))
                .and_then(|row| HostSoftware::from_detail(name, &row.detail))
        })
        .collect();
    Report {
        host: host.to_string(),
        rows,
        scripts,
        freshness,
    }
}

/// Every host that has a software report on file.
pub fn reported_hosts(records: &[Observation]) -> Vec<String> {
    let mut hosts: Vec<String> = records
        .iter()
        .filter_map(|row| row.fact.strip_prefix(REPORT_KIND).map(str::to_string))
        .collect();
    hosts.sort();
    hosts.dedup();
    hosts
}

// ---------------------------------------------------------------------------
// Judging
// ---------------------------------------------------------------------------

/// The release-control product one target is supposed to be running, as a
/// concrete file on that host.
#[derive(Debug, Clone)]
pub struct ProductBinary {
    /// The program's basename, which is how the host reports it.
    pub name: String,
    /// The absolute path the release policy installs it at.
    pub path: String,
    /// `None` when the registry declares no desired release, which is a
    /// different finding from a disagreement.
    pub desired: Option<String>,
}

/// What one target's software report says about the declarations it is supposed
/// to satisfy.
#[derive(Debug, Clone, Default)]
pub struct Finding {
    /// True when this target is in a state an operator has to act on.
    pub failed: bool,
    /// One sentence per disagreement, each naming the host and the exact
    /// disagreement. Empty when there is nothing to say.
    pub sentences: Vec<String>,
}

impl Finding {
    /// The one word a screen sorts and colours on. `ok` is only ever reached by a
    /// fresh report in which every declared program is accounted for.
    pub fn word(&self) -> &'static str {
        if self.failed {
            "failed"
        } else {
            "ok"
        }
    }

    /// The verdict, folded into the report it is about.
    ///
    /// One object rather than a nested one, so a consumer reads `verdict` and
    /// `findings` beside the counts they were computed from. `verdict` and not
    /// `state`: the report already carries a `state` — whether the look
    /// happened — and two different questions under one key is how a screen
    /// comes to colour "nobody looked" as "everything is wrong", or worse, the
    /// other way round.
    pub fn merge_into(&self, report: &mut Value) {
        let Some(object) = report.as_object_mut() else {
            return;
        };
        object.insert("verdict".to_string(), json!(self.word()));
        object.insert("failed".to_string(), json!(self.failed));
        object.insert("findings".to_string(), json!(self.sentences));
    }

    pub fn json(&self) -> Value {
        let mut value = json!({});
        self.merge_into(&mut value);
        value
    }

    fn fail(&mut self, sentence: String) {
        self.failed = true;
        self.sentences.push(sentence);
    }
}

/// Everything wrong with one program, in one sentence, or nothing.
///
/// One sentence per program rather than one per fault: an operator reading a gate
/// wants the row and everything the fleet has against it, and splitting
/// "unmanaged" from "wrong version" into two lines about one file makes the
/// output twice as long without adding a fact.
fn disagreement(host: &str, row: &HostSoftware, declared: Option<&str>) -> Option<String> {
    let mut faults: Vec<String> = Vec::new();
    if !row.is_release() {
        faults.push(format!(
            "its digest {} matches no release artefact Stado published, so it is {}",
            row.short_digest(),
            row.provenance
        ));
    }
    match declared {
        Some(want) if row.version == UNKNOWN => faults.push(format!(
            "it reports no version at all, so the declared {want} cannot be confirmed"
        )),
        Some(want) if want != row.version => faults.push(format!("the fleet declares {want}")),
        _ => {}
    }
    if faults.is_empty() {
        return None;
    }
    Some(format!(
        "{host} runs {} {} at {}: {}",
        row.name,
        row.version,
        row.path,
        faults.join(", and ")
    ))
}

/// Does this host's newest report account for what the fleet declares it runs?
///
/// `declared` is the host's `managed_versions`: name to exact version, the same
/// primitive `service converge` and `host release` judge against. `product` is
/// the release-control binary rolled out to this target, which is declared
/// somewhere else entirely and lives under the product's own install root, so it
/// appears in none of the `managed_versions` entries.
///
/// Every failure here is a state an operator has to act on, and every one of them
/// was previously either invisible or printed beside a zero exit.
pub fn judge(
    report: &Report,
    declared: &BTreeMap<String, String>,
    product: Option<&ProductBinary>,
) -> Finding {
    let mut finding = Finding::default();
    let host = report.host.as_str();

    match &report.freshness {
        Freshness::Never => {
            finding.fail(format!(
                "{host} has never reported what software it runs, so every version claimed for it \
                 is a declaration nothing on the host confirms: run `stado host software {host}`"
            ));
            return finding;
        }
        Freshness::Stale(_) => finding.fail(format!(
            "{host} last reported its software {}, past the window an observation speaks for, so \
             nothing here describes the present: run `stado host software {host}`",
            report.age()
        )),
        Freshness::Fresh(_) => {}
    }
    if report.state() != OBSERVED {
        finding.fail(format!(
            "{host} could not report its software ({}): {}",
            report.state(),
            report.refusal()
        ));
        return finding;
    }

    // The registry's per-binary statement of what this host must run, checked
    // against the bytes. `service converge` makes this comparison on versions
    // alone; the digest half is what tells a delivered build apart from one
    // somebody carried over by hand at the same version number.
    for (name, want) in declared {
        match report.find(name) {
            None => finding.fail(format!(
                "{host} declares {name} {want} and its software report names no {name} program at \
                 all, so the declaration is unconfirmed on the host that carries it"
            )),
            Some(row) => {
                if let Some(sentence) = disagreement(host, row, Some(want)) {
                    finding.fail(sentence);
                }
            }
        }
    }

    // The rollout's own binary. Matched on its declared path first: the product
    // install root is where the release puts it, and a same-named program
    // elsewhere on the host is a different file.
    if let Some(product) = product {
        let found = report
            .rows
            .iter()
            .find(|row| row.path == product.path)
            .or_else(|| report.find(&product.name));
        match found {
            None => finding.fail(format!(
                "{host} reports no {} program at {}, so the desired {} is confirmed nowhere on the \
                 host it rolls out to",
                product.name,
                product.path,
                product.desired.as_deref().unwrap_or("release")
            )),
            Some(row) => {
                if let Some(sentence) = disagreement(host, row, product.desired.as_deref()) {
                    finding.fail(sentence);
                }
            }
        }
    }

    finding
}
