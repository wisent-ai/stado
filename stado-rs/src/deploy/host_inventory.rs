//! `stado host inventory TARGET` — what stado actually installed on one
//! registry host, what that host's forward markers claim, and whether the
//! two agree.
//!
//! NO Python original. This exists because of a real diagnosis that could
//! not be finished with the shipped commands: on `control-host` the
//! marker `$HOME/.stado/forwards/stado-weles-api.url` said
//! `http://127.0.0.1:8766` while the admission API was listening on `8794`.
//! Nothing in the fleet noticed, because nothing in the fleet reads the
//! markers. Finding it took a raw `ssh user@ip '<inline script>'` with a
//! hardcoded address, and answering "does that port still exist" reached
//! for `pgrep -fl` and `printenv` — the two things
//! [`crate::deploy::host_exec`]'s allowlist deliberately does not offer,
//! because process arguments and environments are where the secrets are.
//!
//! The fix is NOT a wider allowlist. `host exec`'s contract is that every
//! entry is a compile-time argv of absolute paths with no operator-supplied
//! path in it; the three facts missing here all need `$HOME`, and an entry
//! taking `$HOME` is an entry taking a path. So this is a separate command
//! with its own, narrower contract:
//!
//! - it takes a registry TARGET NAME and nothing else — no path, no file
//!   name, no port, no pattern. There is no way to point it at something;
//!   it is a fixed question, not a parameterizable probe;
//! - its remote program is one compile-time script with no interpolation
//!   in it at all, run over the shared channel
//!   ([`crate::deploy::host_channel::run_script`]), the same way
//!   `host forward-local` writes its marker;
//! - every value it reads off the host is reduced to a JSON-inert
//!   character set and capped in length on BOTH sides, so a corrupt or
//!   hostile file under `~/.stado` cannot push arbitrary text into an
//!   operator's terminal;
//! - it refuses to follow a symlink, at the managed binaries, the markers
//!   or the vault files, and reports the refusal instead of reading through
//!   it.
//!
//! What it will never show: process arguments, process environments, vault
//! or secret file contents, tokens, or anything read through `lsof` or
//! `pgrep -f`. Listener ownership comes from `netstat -anv -p tcp`, the
//! program `host exec` already justifies as safe precisely because it reads
//! the kernel socket table and no process's argv.
//!
//! The point of the command is the last section, not the first three. A
//! table of markers next to a table of listeners is still homework; the
//! report answers the question directly, per marker and in aggregate:
//! `matched` when something is listening on the port the marker names,
//! `stale` when nothing is. `reconciliation.stale_markers` is the sentence
//! an operator actually needs.
//!
//! The same question is asked of Skarbiec: `vaults` is
//! `$HOME/.stado/*.vault.json`, `vault_sidecars` is everything else under
//! `$HOME/.stado/*.vault*.json` — snapshots, pre-migration copies,
//! `*.acquisitions.json`. Keeping them apart is operational, not tidy: the
//! active vault is state and a sidecar is history, and an operator who
//! confuses the two edits the wrong file.
//!
//! That section is METADATA ONLY, and that is a boundary rather than an
//! omission. It reports that a vault exists, how large it is, its mode, and
//! whether anything but its owner can read it. It never opens one: no
//! ciphertext, no item id, no consumer name, no token. `stat(2)` answers
//! "which vaults are on this host" completely, so nothing here needs
//! `open(2)`, and a diagnostic command that reads secret files is a
//! diagnostic command that leaks them into terminals and logs.
//! `reconciliation.vaults_not_owner_only` is the finding that matters: a
//! vault the group can read is an incident.
//!
//! Reporting drift is not the same as failing on it. A host with a forward
//! that was deliberately torn down is not a broken host, so `status` stays
//! [`OK_STATUS`] whenever the inventory was collected; the drift is in the
//! report, loudly, and the exit status stays usable for "just give me the
//! facts".

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::host_channel;
use super::{DeployError, Runner};
use crate::targets::{ComputeTarget, ServiceDirectory};

/// `status` for an inventory that was collected. Whether it found drift is
/// a question the report answers, not a question the exit status answers.
pub const OK_STATUS: &str = "inventory";

/// The cap, in characters, on every string this command reports.
///
/// The remote script caps its own fields at the same number; this side caps
/// again because the far side is whatever answered the ssh connection, and a
/// guarantee that only holds when the remote behaves is not a guarantee.
pub const MAX_FIELD_CHARS: usize = 200;

/// Appended to a value this side had to clip, so a truncated string is never
/// mistaken for a whole one. Counted inside [`MAX_FIELD_CHARS`].
const ELLIPSIS: &str = "...";

/// A marker the script read successfully. Any other state means the file was
/// refused, not that it was empty.
pub const MARKER_READ: &str = "read";

/// The two sides of a comparison agree. Shared by all three axes: a marker
/// whose port something is listening on, a marker that names the endpoint
/// the registry declares, and a binary at the version the registry
/// requires.
pub const MATCHED: &str = "matched";
/// The marker exists and nothing is listening on the port it names. This is
/// the `8766` / `8794` case that started this command.
pub const STALE: &str = "stale";
/// The marker could not be turned into a port to check: it was refused as a
/// symlink or a non-regular file, or its contents are not a loopback URL.
pub const UNREADABLE: &str = "unreadable";
/// There was nothing to compare against, so no verdict was reached. For a
/// marker: it names a port and the host's socket table was not read. This
/// is not [`STALE`] — an empty listener table would otherwise turn one
/// failed `netstat` into a report that every forward on the host is dead.
/// For a binary: the registry declares a version and the host reported none
/// that could be read, which is not the same finding as the two disagreeing.
pub const UNKNOWN: &str = "unknown";

/// The installed binary is OLDER than the version the registry declares.
///
/// This axis is independent of the two marker axes below it, and it is the
/// one the fleet was blind to: `host inventory` could always read that
/// `operator-host` runs `stado 0.4.392`, and had nothing to say about
/// whether that is the version it is supposed to run.
pub const BEHIND: &str = "behind";
/// The installed binary is NEWER than the version the registry declares.
/// Not a lesser finding than [`BEHIND`]: a host running ahead of the
/// declaration means the declaration was never updated, and the next host
/// brought to the declared version is a host taken backwards.
pub const AHEAD: &str = "ahead";
/// Declared and installed differ, and at least one of them is not three
/// dot-separated numbers, so there is no older-or-newer to report. Saying
/// `mismatched` beats ordering two strings whose ordering is invented.
pub const MISMATCHED: &str = "mismatched";
/// The registry declares nothing for this binary or this marker, so there
/// is no target state to hold the host to. Reported as its own word rather
/// than folded into [`MATCHED`]: an undeclared thing is not a verified
/// thing, and a fleet whose registry is silent should read as unverified.
pub const UNDECLARED: &str = "undeclared";
/// The marker and the registry name DIFFERENT endpoints for this host.
///
/// The second, independent reconciliation axis. A marker can be [`MATCHED`]
/// against the socket table and `disagrees` against the registry at the
/// same time, and that combination is the dangerous one: something is
/// listening, so nothing looks broken, and it is not the endpoint the
/// directory sends consumers to. `skarbiec-weles` on `control-host` is
/// exactly that — the marker says `8895`, something answers on `8895`, and
/// the registry declares `19095`.
pub const DISAGREES: &str = "disagrees";

/// [`ManagedBinary::version_state`] when the host actually answered with a
/// version. Every other state is a reason the `version` field is blank, so
/// it is the only state whose `version` may be compared to a declaration.
pub const VERSION_REPORTED: &str = "reported";

/// The remote script's own sanitizer answered its fixed probe correctly, so
/// every string in the report was reduced by a working sanitizer.
pub const SANITIZER_OK: &str = "ok";
/// The sanitizer did not answer its own probe. Every string in the report is
/// then suspect, which is a host fault to state outright rather than
/// something for an operator to infer from a table of blank names — the
/// exact failure this state exists because of.
pub const SANITIZER_BROKEN: &str = "broken";

/// `netstat -anv -p tcp` answered and its output was parsed.
pub const LISTENERS_READ: &str = "read";
/// `netstat` did not answer. The listener table is empty because it could
/// not be read, not because nothing is listening.
pub const LISTENERS_FAILED: &str = "failed";

/// The cap on how many files each vault section reports.
///
/// The remote script caps at the same number. A `~/.stado` with a thousand
/// files must not produce an unbounded report, and the script counts
/// everything it matched into `vaults_seen` / `vault_sidecars_seen`, so the
/// cap shows up as a number the report states rather than as a silent cut.
pub const MAX_VAULT_FILES: usize = 64;

/// A vault path that is a regular file. Its METADATA was read; its contents
/// were not, and there is no state in which they would be.
pub const VAULT_REGULAR: &str = "regular";
/// A vault path that is a symlink. The link is reported and never followed:
/// the size and mode belong to the link, not to whatever it points at.
pub const VAULT_REFUSED_SYMLINK: &str = "refused_symlink";
/// A vault path that exists and is neither a symlink nor a regular file.
pub const VAULT_REFUSED_NOT_REGULAR: &str = "refused_not_regular";

/// The fixed remote program.
///
/// Nothing is interpolated into it — it is a `&'static str`, not a
/// `format!`, so there is no operator input to escape. Every expansion is
/// quoted, every value passes through `sanitize`, and every external
/// program is named by absolute path the way the recovery and
/// GUI-automation scripts name theirs.
pub const REMOTE_INVENTORY_SCRIPT: &str = r##"set -eu
LC_ALL=C
export LC_ALL

stado_home="$HOME/.stado"
bin_dir="$stado_home/bin"
forward_dir="$stado_home/forwards"
field_limit=200
# The cap on how many files each vault section reports. A directory with a
# thousand files must not produce an unbounded report; what was matched
# beyond the cap is counted, not silently dropped.
vault_limit=64
# A literal newline, for the first-line-only expansions below. Written as a
# quoted line break rather than bash's $'\n' so nothing here needs a dialect.
newline='
'

# Reduce one value to a bounded, JSON-inert token: every character outside a
# conservative allowlist becomes '?', and what is left is cut to field_limit
# characters. Escaping would also work; refusing the dangerous characters
# outright is a guarantee that does not depend on getting the escaping right,
# and it means a corrupt or hostile file under ~/.stado cannot emit quotes,
# backslashes, newlines or unbounded text into this report.
#
# Shell builtins only, and the answer comes back in "sanitized" instead of on
# stdout. That is the whole point of this function's shape, not a style
# choice. It used to be "$(printf | tr -d | tr -c | cut)": a command
# substitution plus three external programs, four forks for every field in
# the report. On a host that had run out of per-user process slots the inner
# forks failed, the subshell died, and the substitution produced the empty
# string with status 128 — and because a command substitution in an argument
# position is invisible to `set -e`, the report went out claiming it had read
# names, modes, versions and marker URLs while emitting "" for every one of
# them. A sanitizer that forks nothing cannot fail that way, and returning
# through a variable is what removes the last fork.
sanitize() {
  sanitize_rest="$1"
  sanitized=""
  sanitize_count=0
  while [ -n "$sanitize_rest" ] && [ "$sanitize_count" -lt "$field_limit" ]; do
    # The leading character, taken by stripping the tail that follows it.
    # Under LC_ALL=C '?' is one byte, so this walks bytes the way `cut -c` did.
    sanitize_tail=${sanitize_rest#?}
    sanitize_char=${sanitize_rest%"$sanitize_tail"}
    sanitize_rest=$sanitize_tail
    case "$sanitize_char" in
      [A-Za-z0-9]|' '|.|,|:|';'|/|@|_|+|=|%|'('|')'|-)
        sanitized="$sanitized$sanitize_char"
        ;;
      *)
        # Control characters land here too. The pipeline this replaced
        # deleted them; mapping them to '?' instead keeps the invariant
        # below true and shows the operator that something was removed.
        sanitized="$sanitized?"
        ;;
    esac
    sanitize_count=$((sanitize_count + 1))
  done
  # A non-empty value must never leave here as an empty field, and if it ever
  # does that is a fault of the host and gets reported as one. The loop
  # appends a character for every character it consumes, so this is reachable
  # only when field_limit or the shell's arithmetic has gone wrong — which is
  # exactly the class of failure that once shipped a report of blanks with
  # every state beside them saying the value had been read. Quiet emptiness is
  # the one outcome this function may not have.
  if [ -z "$sanitized" ] && [ -n "$1" ]; then
    sanitized='?'
    sanitizer_state=broken
  fi
}

# The sanitizer is checked before anything is reported, never trusted. Every
# string below depends on it and on nothing else, so a host where it does not
# do its job says so in a field of its own instead of returning a report full
# of empty strings that reads like a host with no names on it.
# The state is emitted at the end of the payload rather than the start,
# because a fault found while sanitizing the report's own fields has to be
# able to reach the field that reports it.
sanitizer_state=ok
sanitize 'probe-Value_1.2'
if [ "$sanitized" != 'probe-Value_1.2' ]; then
  sanitizer_state=broken
fi
sanitize 'a"b'
if [ "$sanitized" != 'a?b' ]; then
  sanitizer_state=broken
fi
sanitize_long=0123456789
sanitize_long=$sanitize_long$sanitize_long$sanitize_long$sanitize_long
sanitize_long=$sanitize_long$sanitize_long$sanitize_long$sanitize_long
sanitize_long=$sanitize_long$sanitize_long$sanitize_long$sanitize_long
sanitize "$sanitize_long"
if [ "${#sanitized}" -ne "$field_limit" ]; then
  sanitizer_state=broken
fi

if [ -L "$forward_dir" ]; then
  forwards_dir_state=symlink
elif [ -d "$forward_dir" ]; then
  forwards_dir_state=directory
elif [ -e "$forward_dir" ]; then
  forwards_dir_state=not_directory
else
  forwards_dir_state=missing
fi

printf '{"forwards_dir_state":"%s","managed_binaries":[' "$forwards_dir_state"

separator=""
for binary_name in stado skarbiec; do
  binary_path="$bin_dir/$binary_name"
  state=missing
  regular=false
  executable=false
  version_state=missing
  version=""
  # -L first, and never -L then -f: -f follows the link, so testing it first
  # would report a symlink to /etc/passwd as a present regular binary.
  if [ -L "$binary_path" ]; then
    state=symlink
    version_state=refused_symlink
  elif [ -f "$binary_path" ]; then
    state=present
    regular=true
    if [ -x "$binary_path" ]; then
      executable=true
      # `stado --version` answers in one plain line; `skarbiec version`
      # answers with a JSON object whose `version` member is the build. Both
      # answers are read according to their shape, because taking line one
      # unconditionally reported `{` for skarbiec, which the sanitizer then
      # correctly reduced to `?`. A brace is not a version.
      if [ "$binary_name" = stado ]; then
        version_argument=--version
      else
        version_argument=version
      fi
      if version_output=$("$binary_path" "$version_argument" 2>/dev/null); then
        version_rc=0
      else
        version_rc=1
      fi
      if [ "$version_rc" -ne 0 ]; then
        # Present, executable, and it did not answer. On a host out of
        # process slots that is what a failed fork looks like from here, and
        # it is reported as a state rather than as a blank version.
        version_state=version_failed
      elif [ -z "$version_output" ]; then
        version_state=version_empty
      else
        case "$version_output" in
          '{'*)
            case "$version_output" in
              *'"version"'*)
                version_rest=${version_output#*'"version"'}
                version_gap=${version_rest%%'"'*}
                case "$version_rest" in
                  *'"'*)
                    # Only whitespace and the colon may sit between the key
                    # and the opening quote of its value. Anything else means
                    # the next quote belongs to a different member — a null
                    # version, for one — and there is no version to report.
                    case "$version_gap" in
                      *[!:[:space:]]*) ;;
                      *)
                        version_rest=${version_rest#*'"'}
                        version=${version_rest%%'"'*}
                        ;;
                    esac
                    ;;
                esac
                ;;
            esac
            ;;
          *)
            # First line only, without the `| head -n 1` this used to fork
            # for: a plain answer may be followed by build details.
            version=${version_output%%"$newline"*}
            ;;
        esac
        if [ -n "$version" ]; then
          version_state=reported
        else
          # It answered in a shape no version could be read out of. Saying so
          # beats reporting a brace, or a fragment of some other member, as
          # this host's build.
          version_state=version_unparsable
        fi
      fi
    else
      version_state=not_executable
    fi
  elif [ -e "$binary_path" ]; then
    state=not_regular
    version_state=refused_not_regular
  fi
  sanitize "$version"
  printf '%s{"name":"%s","state":"%s","regular_file":%s,"executable":%s,"version_state":"%s","version":"%s"}' \
    "$separator" "$binary_name" "$state" "$regular" "$executable" "$version_state" "$sanitized"
  separator=,
done

printf '],"forwards":['
separator=""
if [ "$forwards_dir_state" = directory ]; then
  for marker_path in "$forward_dir"/*.url; do
    # An unmatched glob stays literal; a dangling symlink fails -e but not -L.
    if [ ! -e "$marker_path" ] && [ ! -L "$marker_path" ]; then
      continue
    fi
    # basename(1) by parameter expansion. Two fewer forks per marker, and the
    # marker name is one of the fields that came back empty when they failed.
    marker_name=${marker_path##*/}
    marker_name=${marker_name%.url}
    url=""
    if [ -L "$marker_path" ]; then
      marker_state=refused_symlink
    elif [ ! -f "$marker_path" ]; then
      marker_state=refused_not_regular
    elif [ ! -r "$marker_path" ]; then
      marker_state=refused_unreadable
    else
      marker_state=read
      # The read builtin, not `head -c 4096 | head -n 1`: one line, no fork,
      # and no pipeline that can die and hand back an empty string. A marker
      # is one short loopback URL, and -n bounds the read so a
      # multi-gigabyte file is never pulled in.
      IFS= read -r -n 4096 url < "$marker_path" || true
      if [ -z "$url" ]; then
        # The file is there and its first line is empty. That is a state,
        # not a URL the other side should be left to guess at.
        marker_state=read_empty
      fi
    fi
    sanitize "$marker_name"
    marker_name_safe=$sanitized
    sanitize "$url"
    printf '%s{"name":"%s","state":"%s","url":"%s"}' \
      "$separator" "$marker_name_safe" "$marker_state" "$sanitized"
    separator=,
  done
fi

# The kernel socket table, nothing else. No lsof, no pgrep -f, no /proc walk:
# the owner is reported as a bare pid, and mapping that pid to a program is
# `stado host exec TARGET -- ps ax -o pid -o ppid -o etime -o comm`, which is
# already approved and already argument-free.
#
# Collected first and judged after, because an empty socket table makes every
# marker on the host look stale, and that is a fleet-wide incident report.
# "netstat did not answer" has to be a state of its own rather than an empty
# list passed off as "nothing is listening".
listeners_state=read
listeners_json=""
if ! netstat_raw=$(/usr/sbin/netstat -anv -p tcp 2>/dev/null); then
  listeners_state=failed
  netstat_raw=""
fi
if [ "$listeners_state" = read ]; then
  if ! listeners_json=$(printf '%s\n' "$netstat_raw" | /usr/bin/awk '
  $6 != "LISTEN" { next }
  {
    address = $4
    parts_count = split(address, parts, ".")
    port = parts[parts_count]
    if (port !~ /^[0-9]+$/) next
    host = substr(address, 1, length(address) - length(port) - 1)
    if (host != "*" && host != "::1" && host !~ /^127\./) next
    if (seen[port]++) next
    pid = 0
    for (field = 7; field <= NF; field++) {
      if ($field ~ /:[0-9]+$/) {
        pid = substr($field, index($field, ":") + 1)
        break
      }
    }
    printf "%s{\"address\":\"%s\",\"port\":%d,\"pid\":%d}", (emitted++ ? "," : ""), host, port + 0, pid + 0
  }
'); then
    listeners_state=failed
    listeners_json=""
  fi
fi
printf '],"listeners":[%s],"listeners_state":"%s","subcommands":[' \
  "$listeners_json" "$listeners_state"

stado_probe=no
if [ ! -L "$bin_dir/stado" ] && [ -f "$bin_dir/stado" ] && [ -x "$bin_dir/stado" ]; then
  stado_probe=yes
fi
separator=""
# Ask for the subcommand's HELP, never run the subcommand: clap exits zero for
# a path it knows and non-zero for one it does not, so the exit code answers
# the version-skew question without the host performing the action.
probe_subcommand() {
  subcommand_name="$*"
  if [ "$stado_probe" = yes ]; then
    if "$bin_dir/stado" "$@" --help >/dev/null 2>&1; then
      subcommand_state=present
    else
      subcommand_rc=$?
      if [ "$subcommand_rc" -ge 126 ]; then
        # 126 and 127 are "could not execute", and a shell that cannot fork
        # reports in the same band. The binary never got to answer, so the
        # answer is not "this subcommand is absent".
        subcommand_state=probe_failed
      else
        subcommand_state=absent
      fi
    fi
  else
    subcommand_state=unavailable
  fi
  sanitize "$subcommand_name"
  printf '%s{"name":"%s","state":"%s"}' \
    "$separator" "$sanitized" "$subcommand_state"
  separator=,
}
probe_subcommand host inventory
probe_subcommand host forward-local
probe_subcommand host exec
probe_subcommand service list
probe_subcommand registry doctor

# Skarbiec vault inventory: METADATA ONLY.
#
# "Which Skarbiec vaults are on this host" is answerable from stat(2), so it
# is answered from stat(2). Nothing below opens a vault, reads a byte of
# ciphertext, counts items, or names a consumer — a vault is a file of
# secrets, and the inventory says one exists and how big it is, never what
# is in it. There is no field in this section that could carry content.
emit_vault_file() {
  vault_path="$1"
  # basename(1) by parameter expansion, for the same reason as the markers.
  vault_name=${vault_path##*/}
  # -L first, and never -L then -f, for the same reason as the binaries: -f
  # follows the link, so a symlink would be reported as a present vault and
  # its target's metadata read instead of the link's.
  if [ -L "$vault_path" ]; then
    vault_state=refused_symlink
  elif [ -f "$vault_path" ]; then
    vault_state=regular
  else
    vault_state=refused_not_regular
  fi
  # Both stat(1) dialects lstat by default, so a symlink reports the link's
  # own size and mode and its target is never touched. BSD form first, GNU
  # form second; neither opens the file.
  if vault_facts=$(/usr/bin/stat -f '%z %Lp' "$vault_path" 2>/dev/null); then
    :
  elif vault_facts=$(/usr/bin/stat -c '%s %a' "$vault_path" 2>/dev/null); then
    :
  else
    vault_facts=""
  fi
  # Split the two fields with parameter expansion instead of two awk forks.
  # bytes is a JSON integer, so a stat that did not answer has to become 0
  # rather than an empty token that breaks the payload, and a mode that did
  # not arrive has to say "unknown" rather than arrive blank.
  vault_bytes=${vault_facts%% *}
  vault_mode=${vault_facts#* }
  case "$vault_bytes" in
    ''|*[!0-9]*) vault_bytes=0 ;;
  esac
  if [ "$vault_mode" = "$vault_facts" ]; then
    vault_mode=unknown
  fi
  # Normalize the two dialects onto one spelling: 0600 and 600 are the same
  # mode, and owner_only must not depend on which stat answered.
  case "$vault_mode" in
    0???) vault_mode=${vault_mode#0} ;;
  esac
  case "$vault_mode" in
    *[!0-7]*) vault_mode=unknown ;;
  esac
  case "$vault_mode" in
    *00) vault_owner_only=true ;;
    *) vault_owner_only=false ;;
  esac
  sanitize "$vault_name"
  vault_name_safe=$sanitized
  sanitize "$vault_mode"
  printf '%s{"name":"%s","state":"%s","bytes":%s,"mode":"%s","owner_only":%s}' \
    "$separator" "$vault_name_safe" "$vault_state" "$vault_bytes" \
    "$sanitized" "$vault_owner_only"
  separator=,
}

printf '],"vaults":['
separator=""
vaults_emitted=0
vaults_seen=0
for vault_path in "$stado_home"/*.vault*.json; do
  # An unmatched glob stays literal; a dangling symlink fails -e but not -L.
  if [ ! -e "$vault_path" ] && [ ! -L "$vault_path" ]; then
    continue
  fi
  # The active vault is exactly "*.vault.json". Everything else matching the
  # wider glob is history — a snapshot, a pre-migration copy, an
  # acquisitions file — and belongs in the other section.
  case "$vault_path" in
    *.vault.json) ;;
    *) continue ;;
  esac
  vaults_seen=$((vaults_seen + 1))
  if [ "$vaults_emitted" -lt "$vault_limit" ]; then
    vaults_emitted=$((vaults_emitted + 1))
    emit_vault_file "$vault_path"
  fi
done

printf '],"vaults_seen":%d,"vault_sidecars":[' "$vaults_seen"
separator=""
sidecars_emitted=0
sidecars_seen=0
for vault_path in "$stado_home"/*.vault*.json; do
  if [ ! -e "$vault_path" ] && [ ! -L "$vault_path" ]; then
    continue
  fi
  case "$vault_path" in
    *.vault.json) continue ;;
  esac
  sidecars_seen=$((sidecars_seen + 1))
  if [ "$sidecars_emitted" -lt "$vault_limit" ]; then
    sidecars_emitted=$((sidecars_emitted + 1))
    emit_vault_file "$vault_path"
  fi
done

printf '],"vault_sidecars_seen":%d,"sanitizer_state":"%s"}\n' \
  "$sidecars_seen" "$sanitizer_state"
"##;

/// One of the two stado-managed binaries under `$HOME/.stado/bin`.
///
/// `version_state` is always an explicit word — `reported`, `missing`,
/// `not_executable`, `version_failed`, `version_empty`,
/// `version_unparsable`, `refused_symlink`, `refused_not_regular`. An empty
/// `version` never has to be interpreted, because the state next to it
/// already says why it is empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedBinary {
    pub name: String,
    pub state: String,
    pub regular_file: bool,
    pub executable: bool,
    pub version_state: String,
    pub version: String,
}

/// One `$HOME/.stado/forwards/*.url` marker, as read (or refused).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForwardMarker {
    pub name: String,
    /// [`MARKER_READ`], or `refused_symlink`, `refused_not_regular`,
    /// `refused_unreadable`, `read_empty`. Every one of those is a reason
    /// `url` is blank, stated where the blank is, so an empty `url` is never
    /// something this side has to interpret.
    pub state: String,
    pub url: String,
}

/// One listening loopback TCP socket and the pid that owns it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Listener {
    /// `127.0.0.1`, `::1`, or `*` for a socket bound to every interface —
    /// which answers on loopback too, and so can satisfy a marker.
    pub address: String,
    pub port: u32,
    pub pid: u32,
}

/// Whether the installed `stado` knows one fixed subcommand path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Subcommand {
    pub name: String,
    /// `present`, `absent`, `unavailable` when there was no usable binary to
    /// ask, or `probe_failed` when the binary was there and never got to
    /// answer — a host out of process slots must not be reported as a host
    /// running an old `stado`.
    pub state: String,
}

/// One `$HOME/.stado/*.vault*.json` file, as METADATA ONLY.
///
/// This is the whole shape of the vault answer, and it is deliberately
/// small. A Skarbiec vault is a file of secrets; "which vaults are on this
/// host" is answerable from `stat(2)`, so it is answered from `stat(2)`.
/// There is no field here that could carry a byte of ciphertext, an item
/// id, a consumer name or a token, because the script never opens the file
/// — not to count, not to validate, not to peek.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VaultFile {
    /// The basename, sanitized by the same shell `sanitize` as every other
    /// value the script reports.
    pub name: String,
    /// [`VAULT_REGULAR`], [`VAULT_REFUSED_SYMLINK`] or
    /// [`VAULT_REFUSED_NOT_REGULAR`].
    pub state: String,
    /// Size in bytes. Typed as an integer, so a `bytes` the host did not
    /// state as a number fails the whole parse instead of arriving as a
    /// quiet zero.
    pub bytes: u64,
    /// Permission bits in octal, e.g. `600`. `unknown` only when the file
    /// disappeared between the glob and the stat.
    pub mode: String,
    /// No group bits and no other bits. A vault the group can read is an
    /// incident, not a cosmetic detail, which is why this is a field of its
    /// own rather than something the operator derives from `mode`.
    pub owner_only: bool,
}

/// Everything the remote script reported, before reconciliation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inventory {
    /// [`SANITIZER_OK`] or [`SANITIZER_BROKEN`], from the script's check of
    /// its own sanitizer against a fixed probe. Every string below went
    /// through that sanitizer, so this is the field that says whether any of
    /// them can be believed.
    pub sanitizer_state: String,
    pub forwards_dir_state: String,
    pub managed_binaries: Vec<ManagedBinary>,
    pub forwards: Vec<ForwardMarker>,
    pub listeners: Vec<Listener>,
    /// [`LISTENERS_READ`] or [`LISTENERS_FAILED`]. An empty `listeners` means
    /// two very different things depending on this, and reconciling markers
    /// against a table that was never read is how one failed `netstat`
    /// becomes a report that every forward on the host is stale.
    pub listeners_state: String,
    pub subcommands: Vec<Subcommand>,
    /// The active vaults: exactly `$HOME/.stado/*.vault.json`.
    pub vaults: Vec<VaultFile>,
    /// How many active vaults matched, including any past
    /// [`MAX_VAULT_FILES`] that `vaults` therefore does not list.
    pub vaults_seen: u64,
    /// Everything else under `$HOME/.stado/*.vault*.json`: snapshots,
    /// pre-migration copies, `*.acquisitions.json`. History, not state.
    pub vault_sidecars: Vec<VaultFile>,
    /// How many sidecars matched, including any past [`MAX_VAULT_FILES`].
    pub vault_sidecars_seen: u64,
}

/// Cap one reported string at [`MAX_FIELD_CHARS`] characters, marking the cut.
fn clamp(value: &mut String) {
    if value.chars().count() <= MAX_FIELD_CHARS {
        return;
    }
    let keep = MAX_FIELD_CHARS - ELLIPSIS.chars().count();
    let end = value
        .char_indices()
        .nth(keep)
        .map_or(value.len(), |(index, _)| index);
    value.truncate(end);
    value.push_str(ELLIPSIS);
}

/// Cap one vault section: the file count first, then every string in it.
///
/// `seen` is raised to the number of entries that actually arrived before
/// the cut, so an over-long list from a misbehaving host is reported as
/// truncated rather than as a section that grew past its own cap.
fn clamp_vault_section(files: &mut Vec<VaultFile>, seen: &mut u64) {
    *seen = (*seen).max(files.len() as u64);
    files.truncate(MAX_VAULT_FILES);
    for file in files {
        clamp(&mut file.name);
        clamp(&mut file.state);
        clamp(&mut file.mode);
    }
}

/// Cap every string in the inventory.
fn clamp_inventory(inventory: &mut Inventory) {
    clamp(&mut inventory.forwards_dir_state);
    clamp(&mut inventory.sanitizer_state);
    clamp(&mut inventory.listeners_state);
    for binary in &mut inventory.managed_binaries {
        clamp(&mut binary.name);
        clamp(&mut binary.state);
        clamp(&mut binary.version_state);
        clamp(&mut binary.version);
    }
    for marker in &mut inventory.forwards {
        clamp(&mut marker.name);
        clamp(&mut marker.state);
        clamp(&mut marker.url);
    }
    for listener in &mut inventory.listeners {
        clamp(&mut listener.address);
    }
    for subcommand in &mut inventory.subcommands {
        clamp(&mut subcommand.name);
        clamp(&mut subcommand.state);
    }
    clamp_vault_section(&mut inventory.vaults, &mut inventory.vaults_seen);
    clamp_vault_section(
        &mut inventory.vault_sidecars,
        &mut inventory.vault_sidecars_seen,
    );
}

/// Parse the script's one line of JSON.
///
/// The LAST line starting with `{` is the payload: a login shell that
/// greets its callers must not be able to turn a healthy host into a parse
/// error.
pub fn parse_inventory(stdout: &str) -> Result<Inventory, DeployError> {
    let payload = stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with('{'))
        .ok_or_else(|| DeployError("host inventory script produced no JSON report".to_string()))?;
    let mut inventory: Inventory = serde_json::from_str(payload).map_err(|error| {
        DeployError(format!(
            "host inventory script did not return the expected JSON: {error}"
        ))
    })?;
    clamp_inventory(&mut inventory);
    Ok(inventory)
}

/// The loopback port a forward marker points at.
///
/// A marker is the one line `stado host forward-local` writes:
/// `http://127.0.0.1:8766`. A marker that is not that shape has no port to
/// reconcile, and saying so beats guessing one.
pub fn marker_port(url: &str) -> Option<u32> {
    let trimmed = url.trim();
    let rest = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))?;
    let authority = rest.split('/').next()?;
    let (host, port) = authority.rsplit_once(':')?;
    if !matches!(host, "127.0.0.1" | "localhost" | "[::1]") {
        return None;
    }
    let port: u32 = port.parse().ok()?;
    if port == u32::MIN || port > u32::from(u16::MAX) {
        return None;
    }
    Some(port)
}

/// One marker's verdict against the listener table: its port, and whether
/// anything is actually listening on it.
///
/// `listeners_state` is not decoration. A marker can only be called
/// [`STALE`] when the socket table it was checked against was actually read;
/// otherwise the verdict is [`UNKNOWN`], because "nothing is listening" and
/// "nothing could be asked" are opposite findings that look identical in an
/// empty `Vec<Listener>`.
pub fn verdict(
    marker: &ForwardMarker,
    listeners: &[Listener],
    listeners_state: &str,
) -> (Option<u32>, &'static str) {
    if marker.state != MARKER_READ {
        return (None, UNREADABLE);
    }
    let Some(port) = marker_port(&marker.url) else {
        return (None, UNREADABLE);
    };
    if listeners_state != LISTENERS_READ {
        return (Some(port), UNKNOWN);
    }
    let listening = listeners.iter().any(|listener| listener.port == port);
    (Some(port), if listening { MATCHED } else { STALE })
}

/// The bare version number inside one [`ManagedBinary::version`] field.
///
/// The two managed binaries answer in two shapes, and this function knows
/// both of them BY NAME rather than sniffing for one:
///
/// - `stado --version` prints `stado 0.5.1`, so the binary's own name
///   followed by whitespace is the one prefix that is ever removed;
/// - `skarbiec version` prints a JSON object, and the remote script has
///   already pulled its `version` member out, so it arrives bare (`0.1.3`).
///
/// Anything else is returned untouched. Guessing a number out of an
/// unfamiliar banner — taking the last word, the first digit run — is how a
/// build string becomes a version, and a wrong version compares cleanly
/// against a declaration and reports the wrong verdict with confidence.
pub fn reported_version<'a>(binary: &str, version: &'a str) -> Option<&'a str> {
    let trimmed = version.trim();
    let bare = match trimmed.strip_prefix(binary) {
        Some(rest) if rest.is_empty() || rest.starts_with(char::is_whitespace) => rest.trim_start(),
        _ => trimmed,
    };
    if bare.is_empty() {
        None
    } else {
        Some(bare)
    }
}

/// A version as three dot-separated numbers, or `None` for everything else.
///
/// Deliberately strict: exactly three components, each a plain integer, no
/// prerelease and no build metadata. A shape this does not recognize is not
/// ordered at all — it falls through to exact equality — because inventing
/// an ordering for `0.5.1-rc2` produces a confident `behind` or `ahead`
/// that nobody can check.
fn version_triple(value: &str) -> Option<(u64, u64, u64)> {
    let mut parts = value.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// One managed binary's verdict against the version the registry declares
/// for this host: [`MATCHED`], [`BEHIND`], [`AHEAD`], [`MISMATCHED`],
/// [`UNDECLARED`] or [`UNKNOWN`].
///
/// Only a binary whose `version_state` is [`VERSION_REPORTED`] has a version
/// to compare. Every other state means the `version` field is blank for a
/// stated reason, and comparing a blank against a declaration would report
/// a missing binary as a version disagreement.
pub fn version_verdict(binary: &ManagedBinary, declared: Option<&str>) -> &'static str {
    let Some(declared) = declared else {
        return UNDECLARED;
    };
    if binary.version_state != VERSION_REPORTED {
        return UNKNOWN;
    }
    let Some(installed) = reported_version(&binary.name, &binary.version) else {
        return UNKNOWN;
    };
    match (version_triple(installed), version_triple(declared)) {
        (Some(installed), Some(declared)) => match installed.cmp(&declared) {
            Ordering::Less => BEHIND,
            Ordering::Equal => MATCHED,
            Ordering::Greater => AHEAD,
        },
        _ if installed == declared.trim() => MATCHED,
        _ => MISMATCHED,
    }
}

/// The endpoint the registry declares for one service ON THIS host.
///
/// `endpoints[target]`, not the active host's endpoint: the question a
/// marker answers is "what should this box's forward point at", and a host
/// standing by for a service still carries a declared endpoint for it. A
/// service the directory does not name, or names without an entry for this
/// host, has nothing declared here and yields `None`.
pub fn declared_endpoint<'a>(
    directory: Option<&'a ServiceDirectory>,
    target: &ComputeTarget,
    service: &str,
) -> Option<&'a str> {
    directory?
        .services
        .get(service)?
        .endpoints
        .get(&target.name)
        .map(|endpoint| endpoint.url.as_str())
}

/// One marker's verdict against the REGISTRY: [`MATCHED`], [`DISAGREES`] or
/// [`UNDECLARED`].
///
/// This is a second axis, not a refinement of [`verdict`]. That one asks
/// whether anything is listening where the marker points; this one asks
/// whether the marker points where the fleet's own directory says it
/// should. They answer independently, and a marker that passes the first
/// and fails the second is the case worth catching: something answers, so
/// the host looks healthy, and consumers resolving through the directory
/// are sent somewhere else entirely.
///
/// When both sides are loopback endpoints the PORT is compared, because
/// `http://localhost:8895` and `http://127.0.0.1:8895` are one endpoint
/// written two ways and calling that a disagreement buries the real ones.
/// When either side is not, exact text is all that can be honestly
/// compared. A marker that could not be read cannot agree with anything, so
/// it lands on [`DISAGREES`]: the registry declares an endpoint this host
/// is not stating.
pub fn declaration_verdict(marker: &ForwardMarker, declared: Option<&str>) -> &'static str {
    let Some(declared) = declared else {
        return UNDECLARED;
    };
    let url = marker.url.trim();
    let agrees = match (marker_port(url), marker_port(declared)) {
        (Some(found), Some(wanted)) => found == wanted,
        _ => url == declared.trim(),
    };
    if agrees {
        MATCHED
    } else {
        DISAGREES
    }
}

/// The inventory as the `--json` report, in `host reboot`'s report shape.
///
/// `directory` is the registry's service directory, when the document
/// carries one. Together with `target.managed_versions` it is the DECLARED
/// state; `inventory` is the observed state; every `*_verdict` field below
/// is one comparison of the two.
pub fn to_report(
    target: &ComputeTarget,
    directory: Option<&ServiceDirectory>,
    inventory: &Inventory,
) -> Map<String, Value> {
    // Axis one: the version each managed binary runs against the version
    // the registry requires of this host.
    let mut binaries = Vec::with_capacity(inventory.managed_binaries.len());
    let mut versions_behind: Vec<&str> = Vec::new();
    let mut versions_ahead: Vec<&str> = Vec::new();
    let mut versions_mismatched: Vec<&str> = Vec::new();
    let mut versions_unjudged: Vec<&str> = Vec::new();
    let mut versions_undeclared: Vec<&str> = Vec::new();
    for binary in &inventory.managed_binaries {
        let declared = target.declared_version(&binary.name);
        let state = version_verdict(binary, declared);
        match state {
            BEHIND => versions_behind.push(&binary.name),
            AHEAD => versions_ahead.push(&binary.name),
            MISMATCHED => versions_mismatched.push(&binary.name),
            UNDECLARED => versions_undeclared.push(&binary.name),
            MATCHED => {}
            _ => versions_unjudged.push(&binary.name),
        }
        binaries.push(json!({
            "name": binary.name,
            "state": binary.state,
            "regular_file": binary.regular_file,
            "executable": binary.executable,
            "version_state": binary.version_state,
            "version": binary.version,
            "declared_version": declared,
            "version_verdict": state,
        }));
    }

    // Axes two and three, per marker and independent of each other: does
    // anything answer where the marker points, and does the marker point
    // where the registry declares this host answers.
    let mut markers = Vec::with_capacity(inventory.forwards.len());
    let mut stale_markers: Vec<&str> = Vec::new();
    let mut disagreeing_markers: Vec<&str> = Vec::new();
    let mut undeclared_markers: Vec<&str> = Vec::new();
    let mut matched = usize::MIN;
    let mut stale = usize::MIN;
    let mut unreadable = usize::MIN;
    let mut unknown = usize::MIN;
    let mut declared_matched = usize::MIN;
    for marker in &inventory.forwards {
        let (port, state) = verdict(marker, &inventory.listeners, &inventory.listeners_state);
        match state {
            MATCHED => matched += 1,
            STALE => {
                stale += 1;
                stale_markers.push(&marker.name);
            }
            UNKNOWN => unknown += 1,
            _ => unreadable += 1,
        }
        let declared_url = declared_endpoint(directory, target, &marker.name);
        let declaration = declaration_verdict(marker, declared_url);
        match declaration {
            DISAGREES => disagreeing_markers.push(&marker.name),
            UNDECLARED => undeclared_markers.push(&marker.name),
            _ => declared_matched += 1,
        }
        markers.push(json!({
            "name": marker.name,
            "state": marker.state,
            "url": marker.url,
            "port": port,
            "reconciliation": state,
            "declared_url": declared_url,
            "declaration_verdict": declaration,
        }));
    }

    // The two vault findings, not the raw table above them. A vault whose
    // group or other bits are set, and a vault path that was refused, are
    // both conclusions an operator should never have to derive by reading
    // a mode column. Only regular files can be judged owner-only: a symlink
    // is lrwxrwxrwx by construction, so listing one here would report the
    // link's permissions as a vault's and drown the real finding.
    let mut vaults_not_owner_only: Vec<&str> = Vec::new();
    let mut vaults_refused: Vec<&str> = Vec::new();
    for vault in &inventory.vaults {
        if vault.state != VAULT_REGULAR {
            vaults_refused.push(&vault.name);
        } else if !vault.owner_only {
            vaults_not_owner_only.push(&vault.name);
        }
    }

    let mut report = host_channel::base_report(target);
    report.insert(
        "sanitizer_state".to_string(),
        json!(inventory.sanitizer_state),
    );
    report.insert(
        "forwards_dir_state".to_string(),
        json!(inventory.forwards_dir_state),
    );
    report.insert("managed_binaries".to_string(), json!(binaries));
    report.insert("forwards".to_string(), json!(markers));
    report.insert("listeners".to_string(), json!(inventory.listeners));
    report.insert(
        "listeners_state".to_string(),
        json!(inventory.listeners_state),
    );
    report.insert("subcommands".to_string(), json!(inventory.subcommands));
    report.insert("vaults".to_string(), json!(inventory.vaults));
    report.insert("vaults_seen".to_string(), json!(inventory.vaults_seen));
    report.insert(
        "vaults_truncated".to_string(),
        json!(inventory.vaults_seen > inventory.vaults.len() as u64),
    );
    report.insert(
        "vault_sidecars".to_string(),
        json!(inventory.vault_sidecars),
    );
    report.insert(
        "vault_sidecars_seen".to_string(),
        json!(inventory.vault_sidecars_seen),
    );
    report.insert(
        "vault_sidecars_truncated".to_string(),
        json!(inventory.vault_sidecars_seen > inventory.vault_sidecars.len() as u64),
    );
    report.insert(
        "reconciliation".to_string(),
        json!({
            "markers": inventory.forwards.len(),
            "matched": matched,
            "stale": stale,
            "unreadable": unreadable,
            "unknown": unknown,
            "stale_markers": stale_markers,
            // The registry axis, counted separately from the listener axis
            // above it on purpose: they are different questions, and one
            // combined "drift" number would hide which of them was answered.
            "declaration_matched": declared_matched,
            "declaration_disagrees": disagreeing_markers.len(),
            "declaration_undeclared": undeclared_markers.len(),
            "disagreeing_markers": disagreeing_markers,
            "undeclared_markers": undeclared_markers,
            "versions_behind": versions_behind,
            "versions_ahead": versions_ahead,
            "versions_mismatched": versions_mismatched,
            "versions_unjudged": versions_unjudged,
            "versions_undeclared": versions_undeclared,
            "vaults_not_owner_only": vaults_not_owner_only,
            "vaults_refused": vaults_refused,
        }),
    );
    report
}

/// Collect the inventory of one already-resolved registry target, against
/// the service directory of the registry it came from.
///
/// Split out from [`inventory_host`] so the whole command can be exercised
/// through the [`Runner`] seam without a registry or a remote host.
pub async fn inventory_target(
    target: &ComputeTarget,
    directory: Option<&ServiceDirectory>,
    runner: &Runner,
) -> Result<Value, DeployError> {
    let output = host_channel::run_script(target, REMOTE_INVENTORY_SCRIPT, runner).await?;
    let parsed = parse_inventory(&output.stdout);
    let mut report = match &parsed {
        Ok(inventory) => to_report(target, directory, inventory),
        Err(_) => host_channel::base_report(target),
    };
    host_channel::finish_report(&mut report, &output, OK_STATUS, "ssh failed");
    // A clean exit with an unreadable payload is its own failure, and a
    // different one from a broken channel: the host answered, and what it
    // answered was not this command's report. A non-zero exit keeps the
    // remote's own last stderr line, which explains more.
    if let (Err(error), true) = (&parsed, output.ok()) {
        report.insert(
            "status".to_string(),
            json!(host_channel::FAILED_STATUS.to_string()),
        );
        report.insert("error".to_string(), json!(error.0));
    }
    Ok(Value::Object(report))
}

/// Collect the inventory of one canonical registry host.
///
/// The whole registry is loaded rather than just the target, because the
/// declaration this command reconciles against lives in two places in the
/// same document: `targets[].managed_versions` and `service_directory`.
/// Comparing a host against a directory from a different read is comparing
/// it against a state that may never have existed.
pub async fn inventory_host(target_name: &str, runner: &Runner) -> Result<Value, DeployError> {
    let registry = crate::targets::fetch_registry_remote()
        .await
        .map_err(|exc| DeployError(exc.to_string()))?;
    let target = host_channel::resolve_target(&registry, target_name)?.clone();
    inventory_target(&target, registry.service_directory.as_ref(), runner).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deploy::{runner_fn, CommandOutput};
    use crate::targets::{DirectoryAuthority, Service, ServiceEndpoint};
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    fn target() -> ComputeTarget {
        ComputeTarget {
            name: "control-host".to_string(),
            kind: "local".to_string(),
            gpu_type: None,
            slots: 1,
            ssh: Some("charles@control-host.local".to_string()),
            region: None,
            spot: false,
            max_concurrent: None,
            team_id: None,
            notes: String::new(),
            hostnames: vec!["control-host.local".to_string()],
            weles: None,
            disk_cleanup: None,
            env_overrides: Default::default(),
            agent_args: Vec::new(),
            vram_gb: None,
            pinned_only: false,
            managed_versions: Default::default(),
            extra: Default::default(),
        }
    }

    /// The incident's own shape: a marker naming 8766 while the listener is
    /// on 8794, next to a marker that does agree with reality.
    fn incident_stdout() -> String {
        json!({
            "sanitizer_state": "ok",
            "listeners_state": "read",
            "forwards_dir_state": "directory",
            "managed_binaries": [
                {
                    "name": "stado",
                    "state": "present",
                    "regular_file": true,
                    "executable": true,
                    "version_state": "reported",
                    "version": "stado 0.4.1",
                },
                {
                    "name": "skarbiec",
                    "state": "missing",
                    "regular_file": false,
                    "executable": false,
                    "version_state": "missing",
                    "version": "",
                },
            ],
            "forwards": [
                {"name": "stado-weles-api", "state": "read", "url": "http://127.0.0.1:8766"},
                {"name": "stado-dashboard", "state": "read", "url": "http://127.0.0.1:9310"},
            ],
            "listeners": [
                {"address": "127.0.0.1", "port": 8794, "pid": 4321},
                {"address": "*", "port": 9310, "pid": 90},
            ],
            "subcommands": [
                {"name": "host inventory", "state": "absent"},
                {"name": "host exec", "state": "present"},
            ],
            "vaults": [
                {
                    "name": "weles.vault.json",
                    "state": "regular",
                    "bytes": 4096,
                    "mode": "600",
                    "owner_only": true,
                },
                {
                    "name": "shared.vault.json",
                    "state": "regular",
                    "bytes": 812,
                    "mode": "644",
                    "owner_only": false,
                },
                {
                    "name": "linked.vault.json",
                    "state": "refused_symlink",
                    "bytes": 21,
                    "mode": "755",
                    "owner_only": false,
                },
            ],
            "vaults_seen": 3,
            "vault_sidecars": [
                {
                    "name": "weles.vault.json.acquisitions.json",
                    "state": "regular",
                    "bytes": 64,
                    "mode": "600",
                    "owner_only": true,
                },
                {
                    "name": "weles.vault.pre-v2.json",
                    "state": "regular",
                    "bytes": 3300,
                    "mode": "600",
                    "owner_only": true,
                },
            ],
            "vault_sidecars_seen": 2,
        })
        .to_string()
    }

    fn ok_runner(stdout: String) -> Runner {
        runner_fn(move |spec| {
            let stdout = stdout.clone();
            async move {
                // The command rides the shared channel with the fixed script
                // on stdin, exactly as `host forward-local` marks its endpoint.
                assert_eq!(spec.stdin.as_deref(), Some(REMOTE_INVENTORY_SCRIPT));
                Ok(CommandOutput {
                    code: 0,
                    stdout,
                    stderr: String::new(),
                })
            }
        })
    }

    /// A [`Runner`] that runs the script the channel handed it, with `HOME`
    /// pointed at a scratch tree instead of the operator's own.
    ///
    /// This is `host_channel::run_script`'s local branch — `/bin/bash -s`
    /// with the script on stdin — so these tests exercise the shipped script
    /// itself rather than a paraphrase of it, and still need no remote host.
    fn scratch_home_runner(home: PathBuf) -> Runner {
        prefixed_home_runner(home, "")
    }

    /// The same runner with `prelude` fed to bash ahead of the shipped
    /// script, for the host conditions no scratch tree can express. The
    /// script itself is still sent byte for byte.
    fn prefixed_home_runner(home: PathBuf, prelude: &'static str) -> Runner {
        runner_fn(move |spec| {
            let home = home.clone();
            async move {
                use tokio::io::AsyncWriteExt;

                let script = spec
                    .stdin
                    .clone()
                    .expect("channel sends the script on stdin");
                let script = format!("{prelude}{script}");
                let mut child = tokio::process::Command::new("/bin/bash")
                    .arg("-s")
                    .env("HOME", &home)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .kill_on_drop(true)
                    .spawn()
                    .map_err(|error| error.to_string())?;
                let mut pipe = child.stdin.take().expect("stdin is piped");
                pipe.write_all(script.as_bytes())
                    .await
                    .map_err(|error| error.to_string())?;
                drop(pipe);
                let output = child
                    .wait_with_output()
                    .await
                    .map_err(|error| error.to_string())?;
                Ok(CommandOutput {
                    code: output.status.code().unwrap_or(-1),
                    stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                })
            }
        })
    }

    /// `$HOME/.stado/{bin,forwards}` under a scratch root.
    fn scratch_tree(home: &Path) -> (PathBuf, PathBuf) {
        let bin = home.join(".stado").join("bin");
        let forwards = home.join(".stado").join("forwards");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(&forwards).unwrap();
        (bin, forwards)
    }

    /// Write an executable stand-in for the host's managed `stado`.
    fn write_fake_stado(path: &Path, body: &str) {
        std::fs::write(path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    /// Run the shipped script against a scratch `HOME` and return the report.
    async fn scratch_report(home: &Path) -> Value {
        inventory_target(&target(), None, &scratch_home_runner(home.to_path_buf()))
            .await
            .unwrap()
    }

    #[test]
    fn parse_inventory_reads_every_section() {
        let inventory = parse_inventory(&incident_stdout()).unwrap();
        assert_eq!(inventory.forwards_dir_state, "directory");
        assert_eq!(inventory.managed_binaries.len(), 2);
        assert_eq!(inventory.managed_binaries[0].version, "stado 0.4.1");
        assert_eq!(inventory.forwards[0].name, "stado-weles-api");
        assert_eq!(inventory.listeners[0].port, 8794);
        assert_eq!(inventory.listeners[0].pid, 4321);
        assert_eq!(inventory.subcommands[1].state, "present");
    }

    #[test]
    fn parse_inventory_ignores_a_chatty_login_shell() {
        let stdout = format!("Welcome to control-host\n{}\n", incident_stdout());
        assert_eq!(
            parse_inventory(&stdout).unwrap().forwards_dir_state,
            "directory"
        );
    }

    #[test]
    fn parse_inventory_refuses_output_that_is_not_the_report() {
        let error = parse_inventory("bash: netstat: command not found\n").unwrap_err();
        assert_eq!(error.0, "host inventory script produced no JSON report");
        let error = parse_inventory("{\"managed_binaries\": 3}\n").unwrap_err();
        assert!(
            error
                .0
                .starts_with("host inventory script did not return the expected JSON:"),
            "unexpected error: {}",
            error.0
        );
    }

    #[test]
    fn marker_port_reads_the_loopback_endpoint_forward_local_writes() {
        assert_eq!(marker_port("http://127.0.0.1:8766"), Some(8766));
        assert_eq!(marker_port("http://127.0.0.1:8766/\n"), Some(8766));
        assert_eq!(marker_port("http://localhost:8794"), Some(8794));
        // Not a loopback endpoint, not a port, or no port at all: there is
        // nothing to reconcile, and inventing one would invent a verdict.
        assert_eq!(marker_port("http://10.0.0.4:8766"), None);
        assert_eq!(marker_port("http://127.0.0.1"), None);
        assert_eq!(marker_port("http://127.0.0.1:0"), None);
        assert_eq!(marker_port("http://127.0.0.1:70000"), None);
        assert_eq!(marker_port("http://127.0.0.1:eight"), None);
        assert_eq!(marker_port(""), None);
    }

    #[test]
    fn marker_whose_port_has_no_listener_is_stale() {
        let inventory = parse_inventory(&incident_stdout()).unwrap();
        let listeners = &inventory.listeners;
        // The incident: the marker says 8766, the admission API is on 8794.
        assert_eq!(
            verdict(&inventory.forwards[0], listeners, LISTENERS_READ),
            (Some(8766), STALE)
        );
        // The second marker names a port that is genuinely bound — through a
        // wildcard socket, which answers on loopback too.
        assert_eq!(
            verdict(&inventory.forwards[1], listeners, LISTENERS_READ),
            (Some(9310), MATCHED)
        );
        // Same marker, same empty verdict material, opposite meaning: a
        // socket table that was never read cannot convict anything of being
        // stale, or one failed netstat reports the whole host as dead.
        assert_eq!(
            verdict(&inventory.forwards[0], &[], LISTENERS_FAILED),
            (Some(8766), UNKNOWN)
        );
        assert_eq!(
            verdict(&inventory.forwards[0], &[], LISTENERS_READ),
            (Some(8766), STALE)
        );
    }

    #[test]
    fn refused_or_unparsable_marker_is_unreadable_not_stale() {
        let listeners = vec![Listener {
            address: "127.0.0.1".to_string(),
            port: 8766,
            pid: 1,
        }];
        let refused = ForwardMarker {
            name: "stado-weles-api".to_string(),
            state: "refused_symlink".to_string(),
            url: String::new(),
        };
        assert_eq!(
            verdict(&refused, &listeners, LISTENERS_READ),
            (None, UNREADABLE)
        );
        let garbage = ForwardMarker {
            name: "stado-weles-api".to_string(),
            state: MARKER_READ.to_string(),
            url: "not a url".to_string(),
        };
        assert_eq!(
            verdict(&garbage, &listeners, LISTENERS_READ),
            (None, UNREADABLE)
        );
    }

    #[test]
    fn reconciliation_counts_every_marker_and_names_the_stale_ones() {
        let mut inventory = parse_inventory(&incident_stdout()).unwrap();
        inventory.forwards.push(ForwardMarker {
            name: "stado-linked".to_string(),
            state: "refused_symlink".to_string(),
            url: String::new(),
        });
        let report = to_report(&target(), None, &inventory);
        assert_eq!(
            report["reconciliation"],
            json!({
                "markers": 3,
                "matched": 1,
                "stale": 1,
                "unreadable": 1,
                "unknown": 0,
                "stale_markers": ["stado-weles-api"],
                // No directory and no declared versions: everything is
                // undeclared, and nothing is silently called matched.
                "declaration_matched": 0,
                "declaration_disagrees": 0,
                "declaration_undeclared": 3,
                "disagreeing_markers": [],
                "undeclared_markers": ["stado-weles-api", "stado-dashboard", "stado-linked"],
                "versions_behind": [],
                "versions_ahead": [],
                "versions_mismatched": [],
                "versions_unjudged": [],
                "versions_undeclared": ["stado", "skarbiec"],
                "vaults_not_owner_only": ["shared.vault.json"],
                "vaults_refused": ["linked.vault.json"],
            })
        );
        assert_eq!(report["forwards"][0]["port"], json!(8766));
        assert_eq!(report["forwards"][0]["reconciliation"], STALE);
        assert_eq!(report["forwards"][1]["reconciliation"], MATCHED);
        assert_eq!(report["forwards"][2]["port"], Value::Null);
    }

    /// The fleet's own service directory, as `stado registry pull` returns
    /// it: `skarbiec-weles` declared on `19095` for `control-host`,
    /// `skarbiec` on `8895`, plus `brama`, which the mini is only STANDING
    /// BY for — `operator-host` is its active host and the mini still
    /// carries a declared endpoint of its own.
    fn mini_directory() -> ServiceDirectory {
        let service = |active_host: &str, endpoints: &[(&str, &str)]| Service {
            placement_profile: None,
            managed_service: None,
            active_host: active_host.to_string(),
            endpoints: endpoints
                .iter()
                .map(|(host, url)| {
                    (
                        (*host).to_string(),
                        ServiceEndpoint {
                            url: (*url).to_string(),
                            extra: Default::default(),
                        },
                    )
                })
                .collect(),
            consumers: Default::default(),
            extra: Default::default(),
        };
        ServiceDirectory {
            authority: DirectoryAuthority {
                target: "control-host".to_string(),
                command: "/Users/charles/.stado/bin/stado".to_string(),
                extra: Default::default(),
            },
            generation: 4,
            services: BTreeMap::from([
                (
                    "skarbiec-weles".to_string(),
                    service(
                        "control-host",
                        &[("control-host", "http://127.0.0.1:19095")],
                    ),
                ),
                (
                    "skarbiec".to_string(),
                    service(
                        "control-host",
                        &[
                            ("control-host", "http://127.0.0.1:8895"),
                            ("operator-host", "http://127.0.0.1:8787"),
                        ],
                    ),
                ),
                (
                    "brama".to_string(),
                    service(
                        "operator-host",
                        &[
                            ("control-host", "http://127.0.0.1:8080"),
                            ("operator-host", "http://127.0.0.1:9080"),
                        ],
                    ),
                ),
            ]),
            extra: Default::default(),
        }
    }

    /// [`target`] with a `managed_versions` declaration on it.
    fn declaring_target(versions: &[(&str, &str)]) -> ComputeTarget {
        let mut target = target();
        target.managed_versions = versions
            .iter()
            .map(|(name, version)| ((*name).to_string(), (*version).to_string()))
            .collect();
        target
    }

    fn managed(name: &str, version_state: &str, version: &str) -> ManagedBinary {
        ManagedBinary {
            name: name.to_string(),
            state: "present".to_string(),
            regular_file: true,
            executable: true,
            version_state: version_state.to_string(),
            version: version.to_string(),
        }
    }

    /// The number is taken out of each binary's OWN answer shape, by name.
    /// Nothing is sniffed: an unfamiliar banner comes back whole, so it
    /// compares as a string instead of being trimmed into a wrong number.
    #[test]
    fn the_version_number_comes_out_of_each_binarys_own_answer_shape() {
        // `stado --version` prints `stado 0.5.1`.
        assert_eq!(reported_version("stado", "stado 0.5.1"), Some("0.5.1"));
        assert_eq!(
            reported_version("stado", "  stado   0.5.1  "),
            Some("0.5.1")
        );
        // `skarbiec version` prints a JSON object; the remote script has
        // already pulled its `version` member, so it arrives bare.
        assert_eq!(reported_version("skarbiec", "0.1.3"), Some("0.1.3"));
        // Only the binary's own name followed by whitespace is stripped.
        assert_eq!(
            reported_version("skarbiec", "stado 0.5.1"),
            Some("stado 0.5.1")
        );
        assert_eq!(
            reported_version("stado", "stadoctl 0.5.1"),
            Some("stadoctl 0.5.1")
        );
        // A name with no number after it is not a version.
        assert_eq!(reported_version("stado", "stado"), None);
        assert_eq!(reported_version("stado", ""), None);
    }

    /// The versions this fleet actually runs, ordered as numbers.
    ///
    /// `0.4.392` against `0.4.5` is the pair that matters: sorted as text,
    /// `392` precedes `5`, so a string comparison calls the newer build the
    /// older one and reports a host that is ahead as behind.
    #[test]
    fn version_verdict_orders_the_versions_the_fleet_runs() {
        let stado = |version: &str| managed("stado", VERSION_REPORTED, version);
        assert_eq!(
            version_verdict(&stado("stado 0.5.1"), Some("0.5.1")),
            MATCHED
        );
        assert_eq!(
            version_verdict(&stado("stado 0.4.392"), Some("0.5.1")),
            BEHIND
        );
        assert_eq!(
            version_verdict(&stado("stado 0.5.1"), Some("0.4.392")),
            AHEAD
        );
        assert_eq!(
            version_verdict(&stado("stado 0.4.392"), Some("0.4.5")),
            AHEAD
        );
        assert_eq!(
            version_verdict(&stado("stado 0.4.5"), Some("0.4.392")),
            BEHIND
        );
        // The bare shape compares the same way.
        assert_eq!(
            version_verdict(
                &managed("skarbiec", VERSION_REPORTED, "0.1.3"),
                Some("0.1.3")
            ),
            MATCHED
        );
    }

    /// A registry that declares nothing is not a host that passed: the
    /// verdict is `undeclared`, never `matched`.
    #[test]
    fn a_binary_the_registry_is_silent_about_is_undeclared() {
        assert_eq!(
            version_verdict(&managed("stado", VERSION_REPORTED, "stado 0.5.1"), None),
            UNDECLARED
        );
    }

    /// Neither side is three numbers, so there is no older or newer to
    /// claim — only whether the two strings are the same.
    #[test]
    fn a_version_that_is_not_three_numbers_is_compared_as_text() {
        let stado = |version: &str| managed("stado", VERSION_REPORTED, version);
        assert_eq!(
            version_verdict(&stado("stado 0.5.1-rc2"), Some("0.5.1")),
            MISMATCHED
        );
        assert_eq!(
            version_verdict(&stado("stado 0.5.1"), Some("0.5.1-rc2")),
            MISMATCHED
        );
        assert_eq!(
            version_verdict(&stado("stado nightly"), Some("nightly")),
            MATCHED
        );
    }

    /// A declaration with nothing readable to compare it against is
    /// `unknown`, which is a different finding from the two disagreeing.
    #[test]
    fn a_binary_that_reported_no_version_is_unjudged_not_mismatched() {
        assert_eq!(
            version_verdict(&managed("stado", "missing", ""), Some("0.5.1")),
            UNKNOWN
        );
        assert_eq!(
            version_verdict(&managed("stado", "version_failed", ""), Some("0.5.1")),
            UNKNOWN
        );
        // A `version` field that survived a failed state is still not a
        // reported version, and must not be compared as one.
        assert_eq!(
            version_verdict(
                &managed("stado", "version_unparsable", "0.5.1"),
                Some("0.5.1")
            ),
            UNKNOWN
        );
    }

    /// The `skarbiec-weles` case, exactly as it stands on the mini: the
    /// marker says `8895`, something IS listening on `8895`, and the
    /// registry declares `19095` for this host. The listener axis says
    /// `matched`; the registry axis must say `disagrees`, independently and
    /// at the same time. This is the drift no health check finds, because
    /// nothing is down.
    #[tokio::test]
    async fn a_marker_can_match_its_listener_and_still_disagree_with_the_registry() {
        let stdout = json!({
            "sanitizer_state": "ok",
            "listeners_state": "read",
            "forwards_dir_state": "directory",
            "managed_binaries": [
                {
                    "name": "stado",
                    "state": "present",
                    "regular_file": true,
                    "executable": true,
                    "version_state": "reported",
                    "version": "stado 0.5.1",
                },
                {
                    "name": "skarbiec",
                    "state": "present",
                    "regular_file": true,
                    "executable": true,
                    "version_state": "reported",
                    "version": "0.1.3",
                },
            ],
            "forwards": [
                {"name": "skarbiec-weles", "state": "read", "url": "http://127.0.0.1:8895"},
                {"name": "skarbiec", "state": "read", "url": "http://127.0.0.1:8785"},
                {"name": "stado-api", "state": "read", "url": "http://127.0.0.1:8765"},
                {"name": "brama", "state": "read", "url": "http://127.0.0.1:8080"},
            ],
            "listeners": [
                {"address": "127.0.0.1", "port": 8895, "pid": 30055},
                {"address": "127.0.0.1", "port": 8765, "pid": 24370},
                {"address": "127.0.0.1", "port": 8080, "pid": 92605},
            ],
            "subcommands": [],
            "vaults": [],
            "vaults_seen": 0,
            "vault_sidecars": [],
            "vault_sidecars_seen": 0,
        })
        .to_string();
        let target = declaring_target(&[("stado", "0.5.1"), ("skarbiec", "0.1.3")]);
        let report = inventory_target(&target, Some(&mini_directory()), &ok_runner(stdout))
            .await
            .unwrap();

        let weles = &report["forwards"][0];
        assert_eq!(weles["name"], "skarbiec-weles");
        assert_eq!(weles["reconciliation"], MATCHED);
        assert_eq!(weles["declared_url"], "http://127.0.0.1:19095");
        assert_eq!(weles["declaration_verdict"], DISAGREES);

        // Stale AND disagreeing: the two axes are independent in both
        // directions.
        let skarbiec = &report["forwards"][1];
        assert_eq!(skarbiec["reconciliation"], STALE);
        assert_eq!(skarbiec["declared_url"], "http://127.0.0.1:8895");
        assert_eq!(skarbiec["declaration_verdict"], DISAGREES);

        // A marker no service in the directory names is undeclared, not a
        // disagreement invented against an endpoint nobody wrote down.
        let api = &report["forwards"][2];
        assert_eq!(api["reconciliation"], MATCHED);
        assert_eq!(api["declared_url"], Value::Null);
        assert_eq!(api["declaration_verdict"], UNDECLARED);

        // `brama` is ACTIVE on operator-host, which declares `9080`. This
        // host is standing by and declares `8080` of its own, and that is
        // the endpoint its marker is held to. Reconciling a standby host
        // against the active host's endpoint would call every correct
        // standby marker a disagreement.
        let brama = &report["forwards"][3];
        assert_eq!(brama["name"], "brama");
        assert_eq!(brama["declared_url"], "http://127.0.0.1:8080");
        assert_eq!(brama["declaration_verdict"], MATCHED);

        let summary = &report["reconciliation"];
        assert_eq!(summary["markers"], 4);
        assert_eq!(summary["matched"], 3);
        assert_eq!(summary["declaration_disagrees"], 2);
        assert_eq!(summary["declaration_matched"], 1);
        assert_eq!(summary["declaration_undeclared"], 1);
        assert_eq!(
            summary["disagreeing_markers"],
            json!(["skarbiec-weles", "skarbiec"])
        );
        assert_eq!(summary["undeclared_markers"], json!(["stado-api"]));

        // The version axis on the same report: both binaries are at the
        // versions the registry declares, and each row carries the
        // declaration it was judged against.
        assert_eq!(report["managed_binaries"][0]["declared_version"], "0.5.1");
        assert_eq!(report["managed_binaries"][0]["version_verdict"], MATCHED);
        assert_eq!(report["managed_binaries"][1]["declared_version"], "0.1.3");
        assert_eq!(report["managed_binaries"][1]["version_verdict"], MATCHED);
        assert_eq!(summary["versions_behind"], json!([]));
        assert_eq!(summary["versions_undeclared"], json!([]));
    }

    /// The other real host: `operator-host` runs `stado 0.4.392` while the
    /// fleet declares `0.5.1`. The report must name it, in the summary an
    /// operator reads, not only in the per-binary row.
    #[test]
    fn a_host_behind_the_declaration_is_named_in_the_summary() {
        let mut inventory = parse_inventory(&incident_stdout()).unwrap();
        inventory.managed_binaries = vec![
            managed("stado", VERSION_REPORTED, "stado 0.4.392"),
            managed("skarbiec", "missing", ""),
        ];
        let target = declaring_target(&[("stado", "0.5.1"), ("skarbiec", "0.1.3")]);
        let report = to_report(&target, None, &inventory);
        let summary = &report["reconciliation"];
        assert_eq!(summary["versions_behind"], json!(["stado"]));
        assert_eq!(summary["versions_unjudged"], json!(["skarbiec"]));
        assert_eq!(summary["versions_undeclared"], json!([]));
        assert_eq!(report["managed_binaries"][0]["version_verdict"], BEHIND);
        assert_eq!(report["managed_binaries"][0]["declared_version"], "0.5.1");
    }

    /// The same endpoint written two ways is one endpoint. Reporting
    /// `http://localhost:8895` against a declared `http://127.0.0.1:8895` as
    /// a disagreement buries the real ones under spelling.
    #[test]
    fn loopback_spelling_is_not_a_disagreement_but_a_different_port_is() {
        let marker = |url: &str| ForwardMarker {
            name: "skarbiec-weles".to_string(),
            state: MARKER_READ.to_string(),
            url: url.to_string(),
        };
        let declared = Some("http://127.0.0.1:8895");
        assert_eq!(
            declaration_verdict(&marker("http://localhost:8895"), declared),
            MATCHED
        );
        assert_eq!(
            declaration_verdict(&marker("http://127.0.0.1:8895/"), declared),
            MATCHED
        );
        assert_eq!(
            declaration_verdict(&marker("http://127.0.0.1:19095"), declared),
            DISAGREES
        );
        // Neither side is a loopback endpoint: exact text is all that can
        // be compared, and a marker that could not be read cannot agree.
        assert_eq!(
            declaration_verdict(&marker("not a url"), Some("not a url")),
            MATCHED
        );
        assert_eq!(
            declaration_verdict(
                &ForwardMarker {
                    name: "skarbiec-weles".to_string(),
                    state: "refused_symlink".to_string(),
                    url: String::new(),
                },
                declared
            ),
            DISAGREES
        );
    }

    #[test]
    fn oversized_field_is_clipped_and_the_clip_is_visible() {
        let stdout = json!({
            "sanitizer_state": "ok",
            "listeners_state": "read",
            "forwards_dir_state": "directory",
            "managed_binaries": [{
                "name": "stado",
                "state": "present",
                "regular_file": true,
                "executable": true,
                "version_state": "reported",
                "version": "v".repeat(5000),
            }],
            "forwards": [{
                "name": "n".repeat(4096),
                "state": "read",
                "url": "http://127.0.0.1:8766",
            }],
            "listeners": [],
            "subcommands": [],
            "vaults": [{
                "name": "v".repeat(4096),
                "state": "regular",
                "bytes": 4096,
                "mode": "m".repeat(4096),
                "owner_only": true,
            }],
            "vaults_seen": 1,
            "vault_sidecars": [],
            "vault_sidecars_seen": 0,
        })
        .to_string();
        let inventory = parse_inventory(&stdout).unwrap();
        let version = &inventory.managed_binaries[0].version;
        assert_eq!(version.chars().count(), MAX_FIELD_CHARS);
        assert!(version.ends_with(ELLIPSIS), "clip is not marked: {version}");
        assert_eq!(inventory.forwards[0].name.chars().count(), MAX_FIELD_CHARS);
        // A short value is never touched, ellipsis included.
        assert_eq!(inventory.forwards[0].url, "http://127.0.0.1:8766");
        assert_eq!(inventory.vaults[0].name.chars().count(), MAX_FIELD_CHARS);
        assert_eq!(inventory.vaults[0].mode.chars().count(), MAX_FIELD_CHARS);
    }

    #[tokio::test]
    async fn inventory_target_reports_drift_without_failing_the_command() {
        let report = inventory_target(&target(), None, &ok_runner(incident_stdout()))
            .await
            .unwrap();
        assert_eq!(report["target"], "control-host");
        assert_eq!(report["status"], OK_STATUS);
        assert_eq!(report["exit_code"], 0);
        assert!(report.get("error").is_none());
        assert_eq!(report["reconciliation"]["stale"], 1);
        assert_eq!(
            report["reconciliation"]["stale_markers"],
            json!(["stado-weles-api"])
        );
        assert_eq!(report["managed_binaries"][1]["version_state"], "missing");
        assert_eq!(report["subcommands"][0]["state"], "absent");
    }

    #[tokio::test]
    async fn clean_exit_with_an_unreadable_payload_is_a_failure() {
        let report = inventory_target(&target(), None, &ok_runner("nothing here\n".to_string()))
            .await
            .unwrap();
        assert_eq!(report["status"], host_channel::FAILED_STATUS);
        assert_eq!(
            report["error"],
            "host inventory script produced no JSON report"
        );
        assert!(report.get("reconciliation").is_none());
    }

    #[tokio::test]
    async fn nonzero_exit_keeps_the_remote_last_stderr_line() {
        let runner = runner_fn(|_spec| async move {
            Ok(CommandOutput {
                code: 255,
                stdout: String::new(),
                stderr: "ssh: connect to host port 22: Host is down\n".to_string(),
            })
        });
        let report = inventory_target(&target(), None, &runner).await.unwrap();
        assert_eq!(report["status"], host_channel::FAILED_STATUS);
        assert_eq!(report["exit_code"], 255);
        assert_eq!(
            report["error"],
            "ssh: connect to host port 22: Host is down"
        );
    }

    // -----------------------------------------------------------------
    // The shipped script itself, run against a scratch $HOME. No ssh, no
    // registry, no stado: /bin/bash reading REMOTE_INVENTORY_SCRIPT on
    // stdin, which is byte for byte what the channel sends.
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn script_reports_an_empty_host_as_explicit_states() {
        let home = tempfile::tempdir().unwrap();
        let report = scratch_report(home.path()).await;
        assert_eq!(report["status"], OK_STATUS);
        assert_eq!(report["forwards_dir_state"], "missing");
        for binary in report["managed_binaries"].as_array().unwrap() {
            assert_eq!(binary["state"], "missing");
            // The absent binary is a state, not a blank version string.
            assert_eq!(binary["version_state"], "missing");
            assert_eq!(binary["version"], "");
            assert_eq!(binary["executable"], false);
        }
        // With no binary to ask, skew is unknown rather than answered "absent".
        for subcommand in report["subcommands"].as_array().unwrap() {
            assert_eq!(subcommand["state"], "unavailable");
        }
        assert_eq!(report["reconciliation"]["markers"], 0);
    }

    #[tokio::test]
    async fn script_refuses_a_symlinked_marker_and_still_reads_the_real_one() {
        let home = tempfile::tempdir().unwrap();
        let (_bin, forwards) = scratch_tree(home.path());
        std::fs::write(
            forwards.join("stado-weles-api.url"),
            "http://127.0.0.1:8766\n",
        )
        .unwrap();
        std::os::unix::fs::symlink("/etc/passwd", forwards.join("stado-linked.url")).unwrap();

        let report = scratch_report(home.path()).await;
        assert_eq!(report["forwards_dir_state"], "directory");
        let markers = report["forwards"].as_array().unwrap();
        let linked = markers
            .iter()
            .find(|marker| marker["name"] == "stado-linked")
            .expect("the symlink is reported, not skipped");
        assert_eq!(linked["state"], "refused_symlink");
        assert_eq!(linked["url"], "");
        assert_eq!(linked["reconciliation"], UNREADABLE);
        let real = markers
            .iter()
            .find(|marker| marker["name"] == "stado-weles-api")
            .expect("the regular marker is read");
        assert_eq!(real["state"], MARKER_READ);
        assert_eq!(real["url"], "http://127.0.0.1:8766");
        assert_eq!(real["port"], 8766);
    }

    #[tokio::test]
    async fn script_bounds_and_neutralizes_a_hostile_marker() {
        let home = tempfile::tempdir().unwrap();
        let (_bin, forwards) = scratch_tree(home.path());
        std::fs::write(
            forwards.join("oversized.url"),
            format!("http://127.0.0.1:8766{}\n", "A".repeat(9000)),
        )
        .unwrap();
        std::fs::write(
            forwards.join("hostile.url"),
            "http://127.0.0.1:8766\",\"injected\":\"yes\nsecond line\n",
        )
        .unwrap();

        let report = scratch_report(home.path()).await;
        let markers = report["forwards"].as_array().unwrap();
        let oversized = markers
            .iter()
            .find(|marker| marker["name"] == "oversized")
            .unwrap();
        let url = oversized["url"].as_str().unwrap();
        assert!(
            url.chars().count() <= MAX_FIELD_CHARS,
            "unbounded marker reached the report: {} chars",
            url.chars().count()
        );
        let hostile = markers
            .iter()
            .find(|marker| marker["name"] == "hostile")
            .unwrap();
        let url = hostile["url"].as_str().unwrap();
        assert!(!url.contains('"'), "quote survived sanitizing: {url}");
        assert!(!url.contains("second line"), "second line leaked: {url}");
        // The escape attempt was `..."` followed by a key of its own. The
        // structs deny unknown fields, so a quote that survived sanitizing
        // would not quietly add a field — it would fail the whole parse.
        assert_eq!(report["status"], OK_STATUS);
        assert_eq!(url, "http://127.0.0.1:8766?,?injected?:?yes");
        // A marker whose contents are not a loopback URL has no port to
        // reconcile, so it is unreadable rather than silently stale.
        assert_eq!(hostile["reconciliation"], UNREADABLE);
    }

    #[tokio::test]
    async fn script_reads_versions_and_detects_subcommand_skew() {
        let home = tempfile::tempdir().unwrap();
        let (bin, _forwards) = scratch_tree(home.path());
        write_fake_stado(
            &bin.join("stado"),
            "#!/bin/bash\ncase \"$*\" in\n  '--version') echo 'stado 9.9.9-test' ;;\n  \
             'host exec --help') ;;\n  *) exit 2 ;;\nesac\n",
        );
        // Present, a regular file, and not executable: three separate facts.
        std::fs::write(bin.join("skarbiec"), "not executable\n").unwrap();
        std::fs::set_permissions(bin.join("skarbiec"), std::fs::Permissions::from_mode(0o600))
            .unwrap();

        let report = scratch_report(home.path()).await;
        let binaries = report["managed_binaries"].as_array().unwrap();
        assert_eq!(binaries[0]["state"], "present");
        assert_eq!(binaries[0]["executable"], true);
        assert_eq!(binaries[0]["version_state"], "reported");
        assert_eq!(binaries[0]["version"], "stado 9.9.9-test");
        assert_eq!(binaries[1]["state"], "present");
        assert_eq!(binaries[1]["regular_file"], true);
        assert_eq!(binaries[1]["executable"], false);
        assert_eq!(binaries[1]["version_state"], "not_executable");

        let subcommands = report["subcommands"].as_array().unwrap();
        let state = |name: &str| {
            subcommands
                .iter()
                .find(|entry| entry["name"] == name)
                .unwrap_or_else(|| panic!("{name} is probed"))["state"]
                .clone()
        };
        assert_eq!(state("host exec"), "present");
        assert_eq!(state("host inventory"), "absent");
        assert_eq!(state("registry doctor"), "absent");
    }

    #[tokio::test]
    async fn script_does_not_follow_a_symlinked_binary() {
        let home = tempfile::tempdir().unwrap();
        let (bin, _forwards) = scratch_tree(home.path());
        std::os::unix::fs::symlink("/bin/echo", bin.join("stado")).unwrap();

        let report = scratch_report(home.path()).await;
        let stado = &report["managed_binaries"][0];
        assert_eq!(stado["state"], "symlink");
        assert_eq!(stado["regular_file"], false);
        assert_eq!(stado["executable"], false);
        assert_eq!(stado["version_state"], "refused_symlink");
        assert_eq!(stado["version"], "");
        // A link is not a binary this command will interrogate, so skew is
        // unanswerable rather than answered from whatever it points at.
        assert_eq!(report["subcommands"][0]["state"], "unavailable");
    }

    /// The whole command end to end against a real kernel socket table: one
    /// marker naming a port that is genuinely bound, one naming a port that
    /// is not. This is the `8766` / `8794` divergence reproduced locally.
    #[tokio::test]
    async fn script_reconciles_markers_against_a_live_socket_table() {
        let home = tempfile::tempdir().unwrap();
        let (_bin, forwards) = scratch_tree(home.path());
        let live = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let live_port = live.local_addr().unwrap().port();
        let dead_port = {
            let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            socket.local_addr().unwrap().port()
        };
        std::fs::write(
            forwards.join("live.url"),
            format!("http://127.0.0.1:{live_port}\n"),
        )
        .unwrap();
        std::fs::write(
            forwards.join("dead.url"),
            format!("http://127.0.0.1:{dead_port}\n"),
        )
        .unwrap();

        let report = scratch_report(home.path()).await;
        let markers = report["forwards"].as_array().unwrap();
        let marker = |name: &str| {
            markers
                .iter()
                .find(|marker| marker["name"] == name)
                .unwrap_or_else(|| panic!("{name} is reported"))
        };
        assert_eq!(marker("live")["port"], live_port);
        assert_eq!(marker("dead")["port"], dead_port);
        if !std::path::Path::new("/usr/sbin/netstat").exists() {
            // Off the macOS fleet this command targets there is no socket
            // table to read. The contract that still holds is that the report
            // is complete and honest about it, never partially written.
            assert!(report["listeners"].as_array().unwrap().is_empty());
            assert_eq!(report["reconciliation"]["markers"], 2);
            return;
        }
        assert_eq!(marker("live")["reconciliation"], MATCHED);
        assert_eq!(marker("dead")["reconciliation"], STALE);
        assert_eq!(
            report["reconciliation"],
            json!({
                "markers": 2,
                "matched": 1,
                "stale": 1,
                "unreadable": 0,
                "unknown": 0,
                "stale_markers": ["dead"],
                "declaration_matched": 0,
                "declaration_disagrees": 0,
                "declaration_undeclared": 2,
                "disagreeing_markers": [],
                "undeclared_markers": ["dead", "live"],
                "versions_behind": [],
                "versions_ahead": [],
                "versions_mismatched": [],
                "versions_unjudged": [],
                "versions_undeclared": ["stado", "skarbiec"],
                "vaults_not_owner_only": [],
                "vaults_refused": [],
            })
        );
        let owner = report["listeners"]
            .as_array()
            .unwrap()
            .iter()
            .find(|listener| listener["port"] == live_port)
            .expect("the bound port is in the socket table");
        assert_eq!(owner["pid"], std::process::id());
        drop(live);
    }

    // -----------------------------------------------------------------
    // The Skarbiec vault sections. The load-bearing assertion in this block
    // is the negative one: whatever else changes, no vault content may
    // reach the report.
    // -----------------------------------------------------------------

    /// Put in a scratch vault so any read of it would be unmistakable in the
    /// output. Nothing legitimate can carry this string out of the host.
    const LEAK_MARKER: &str = "SEKRET-NIE-MA-PRAWA-WYJSC";

    /// A vault body that looks like the real thing and is entirely poison.
    fn vault_body() -> String {
        format!("{{\"items\":{{\"weles-api-token\":\"{LEAK_MARKER}\"}}}}\n")
    }

    /// Write one file directly under `$HOME/.stado` with an exact mode.
    fn write_vault(home: &Path, name: &str, body: &str, mode: u32) {
        let path = home.join(".stado").join(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    /// One named entry out of a report section.
    fn entry<'a>(report: &'a Value, section: &str, name: &str) -> &'a Value {
        report[section]
            .as_array()
            .unwrap_or_else(|| panic!("{section} is an array"))
            .iter()
            .find(|file| file["name"] == name)
            .unwrap_or_else(|| panic!("{name} is reported in {section}"))
    }

    /// The names of one report section, in the order the host reported them.
    fn names(report: &Value, section: &str) -> Vec<String> {
        report[section]
            .as_array()
            .unwrap_or_else(|| panic!("{section} is an array"))
            .iter()
            .map(|file| file["name"].as_str().unwrap().to_string())
            .collect()
    }

    /// The script's raw stdout, before this side parses anything: the only
    /// place to prove what the host was allowed to emit at all.
    async fn scratch_stdout(home: &Path) -> String {
        host_channel::run_script(
            &target(),
            REMOTE_INVENTORY_SCRIPT,
            &scratch_home_runner(home.to_path_buf()),
        )
        .await
        .unwrap()
        .stdout
    }

    #[tokio::test]
    async fn vault_sections_parse_and_name_the_two_findings() {
        let report = inventory_target(&target(), None, &ok_runner(incident_stdout()))
            .await
            .unwrap();
        assert_eq!(
            names(&report, "vaults"),
            ["weles.vault.json", "shared.vault.json", "linked.vault.json"]
        );
        let weles = entry(&report, "vaults", "weles.vault.json");
        assert_eq!(weles["state"], VAULT_REGULAR);
        assert_eq!(weles["bytes"], 4096);
        assert_eq!(weles["mode"], "600");
        assert_eq!(weles["owner_only"], true);
        assert_eq!(
            entry(&report, "vaults", "linked.vault.json")["state"],
            VAULT_REFUSED_SYMLINK
        );

        // History, not state: the acquisitions file and the pre-migration
        // copy are never mixed into the active list.
        assert_eq!(
            names(&report, "vault_sidecars"),
            [
                "weles.vault.json.acquisitions.json",
                "weles.vault.pre-v2.json"
            ]
        );
        assert_eq!(report["vaults_seen"], 3);
        assert_eq!(report["vaults_truncated"], false);
        assert_eq!(report["vault_sidecars_seen"], 2);
        assert_eq!(report["vault_sidecars_truncated"], false);

        // The conclusions, not the mode column. A refused path is counted
        // once, as refused, and never also as "not owner only" — a symlink
        // is lrwxrwxrwx by construction and would drown the real finding.
        assert_eq!(
            report["reconciliation"]["vaults_not_owner_only"],
            json!(["shared.vault.json"])
        );
        assert_eq!(
            report["reconciliation"]["vaults_refused"],
            json!(["linked.vault.json"])
        );
        // Vault drift is reported, not punished, exactly like marker drift.
        assert_eq!(report["status"], OK_STATUS);
    }

    #[tokio::test]
    async fn script_separates_the_active_vault_from_its_sidecars() {
        let home = tempfile::tempdir().unwrap();
        scratch_tree(home.path());
        let body = vault_body();
        write_vault(home.path(), "weles.vault.json", &body, 0o600);
        write_vault(home.path(), "shared.vault.json", "{}\n", 0o644);
        write_vault(
            home.path(),
            "weles.vault.json.acquisitions.json",
            "[]\n",
            0o600,
        );
        write_vault(home.path(), "weles.vault.pre-v2.json", &body, 0o600);
        std::os::unix::fs::symlink(
            home.path().join(".stado").join("weles.vault.json"),
            home.path().join(".stado").join("linked.vault.json"),
        )
        .unwrap();

        let report = scratch_report(home.path()).await;
        assert_eq!(
            names(&report, "vaults"),
            ["linked.vault.json", "shared.vault.json", "weles.vault.json"]
        );
        assert_eq!(
            names(&report, "vault_sidecars"),
            [
                "weles.vault.json.acquisitions.json",
                "weles.vault.pre-v2.json"
            ]
        );

        let weles = entry(&report, "vaults", "weles.vault.json");
        assert_eq!(weles["state"], VAULT_REGULAR);
        assert_eq!(weles["mode"], "600");
        assert_eq!(weles["owner_only"], true);
        assert_eq!(weles["bytes"], body.len());

        let shared = entry(&report, "vaults", "shared.vault.json");
        assert_eq!(shared["mode"], "644");
        assert_eq!(shared["owner_only"], false);

        // The link is reported, never followed: its size is the length of
        // the path it holds, not the size of the vault it points at.
        let linked = entry(&report, "vaults", "linked.vault.json");
        assert_eq!(linked["state"], VAULT_REFUSED_SYMLINK);
        assert_ne!(linked["bytes"], json!(body.len()));

        assert_eq!(
            report["reconciliation"]["vaults_not_owner_only"],
            json!(["shared.vault.json"])
        );
        assert_eq!(
            report["reconciliation"]["vaults_refused"],
            json!(["linked.vault.json"])
        );
    }

    #[tokio::test]
    async fn host_without_a_vault_reports_empty_lists_not_an_error() {
        let home = tempfile::tempdir().unwrap();
        scratch_tree(home.path());
        let report = scratch_report(home.path()).await;
        assert_eq!(report["status"], OK_STATUS);
        assert_eq!(report["vaults"], json!([]));
        assert_eq!(report["vault_sidecars"], json!([]));
        assert_eq!(report["vaults_seen"], 0);
        assert_eq!(report["vault_sidecars_seen"], 0);
        assert_eq!(report["vaults_truncated"], false);
        assert_eq!(report["vault_sidecars_truncated"], false);
        assert_eq!(report["reconciliation"]["vaults_not_owner_only"], json!([]));
        assert_eq!(report["reconciliation"]["vaults_refused"], json!([]));
    }

    #[tokio::test]
    async fn vault_files_past_the_cap_are_reported_as_truncated() {
        let home = tempfile::tempdir().unwrap();
        scratch_tree(home.path());
        let extra = 6;
        for index in 0..MAX_VAULT_FILES + extra {
            write_vault(
                home.path(),
                &format!("v{index:03}.vault.json"),
                "{}\n",
                0o600,
            );
        }

        let report = scratch_report(home.path()).await;
        assert_eq!(report["vaults"].as_array().unwrap().len(), MAX_VAULT_FILES);
        // The cut is a number the report states, never a silent shortfall.
        assert_eq!(report["vaults_seen"], MAX_VAULT_FILES + extra);
        assert_eq!(report["vaults_truncated"], true);
        assert_eq!(report["vault_sidecars_truncated"], false);
        assert_eq!(report["status"], OK_STATUS);

        // The cap is the host's, not only this side's: an unbounded list
        // must never be built and pushed across the ssh channel at all, so
        // assert it on the script's own stdout, before any clamping here.
        let stdout = scratch_stdout(home.path()).await;
        let line = stdout
            .lines()
            .rev()
            .map(str::trim)
            .find(|line| line.starts_with('{'))
            .expect("the script emits its report");
        let raw: Value = serde_json::from_str(line).unwrap();
        assert_eq!(raw["vaults"].as_array().unwrap().len(), MAX_VAULT_FILES);
        assert_eq!(raw["vaults_seen"], MAX_VAULT_FILES + extra);
    }

    /// The one that matters. A vault is a file of secrets; this section
    /// reports that it exists and nothing about what is in it. If the script
    /// ever grows a read of a vault, this test is what catches it.
    #[tokio::test]
    async fn script_never_emits_a_byte_of_vault_content() {
        let home = tempfile::tempdir().unwrap();
        scratch_tree(home.path());
        let body = vault_body();
        write_vault(home.path(), "weles.vault.json", &body, 0o600);
        write_vault(home.path(), "weles.vault.pre-v2.json", &body, 0o600);
        std::os::unix::fs::symlink(
            home.path().join(".stado").join("weles.vault.json"),
            home.path().join(".stado").join("linked.vault.json"),
        )
        .unwrap();

        let stdout = scratch_stdout(home.path()).await;
        assert!(
            !stdout.contains(LEAK_MARKER),
            "vault content reached the script's stdout: {stdout}"
        );
        let report = scratch_report(home.path()).await;
        assert!(
            !report.to_string().contains(LEAK_MARKER),
            "vault content reached a report field: {report}"
        );
        // Not vacuous: the vault really is there, really was inventoried,
        // and really does contain the marker on disk.
        assert_eq!(
            entry(&report, "vaults", "weles.vault.json")["bytes"],
            body.len()
        );
        assert!(
            std::fs::read_to_string(home.path().join(".stado").join("weles.vault.json"))
                .unwrap()
                .contains(LEAK_MARKER)
        );
    }
    // -----------------------------------------------------------------
    // The sanitizer, and the incident it caused. On `control-host` the
    // host had run out of per-user process slots, so the four forks the old
    // `$(printf | tr | tr | cut)` needed could not all be had; the
    // substitution returned the empty string with status 128, and `set -e`
    // cannot see a failed substitution in an argument position. Every
    // sanitized field went out as `""` — marker names, marker URLs, vault
    // names, vault modes, versions, subcommand names — while the states
    // beside them still said the values had been read, and all seven
    // markers were counted `unreadable` because their URLs had no port left
    // in them. The tests below are the ones that would have caught it: they
    // assert the sanitizer's output, not that a field parsed.
    // -----------------------------------------------------------------

    /// Every external program the script names, shadowed by a shell function
    /// that cannot run, plus an empty `PATH`. This is the fork-starved host
    /// reproduced deterministically — `tr` and `cut` included, the two the
    /// sanitizer used to be built out of.
    const NO_EXTERNAL_PROGRAMS: &str = "\
/usr/bin/tr() { return 127; }
/usr/bin/cut() { return 127; }
/usr/bin/basename() { return 127; }
/usr/bin/head() { return 127; }
/usr/bin/awk() { return 127; }
/usr/bin/stat() { return 127; }
/usr/sbin/netstat() { return 127; }
PATH=
export PATH
";

    #[tokio::test]
    async fn script_sanitizer_maps_every_character_class_and_cuts_at_the_limit() {
        let home = tempfile::tempdir().unwrap();
        let (_bin, forwards) = scratch_tree(home.path());
        // Allowlisted prefix, then one of each thing that must not survive —
        // a quote, a backslash, a C0 control byte, DEL, two characters
        // outside the set, a tab — then more allowlisted characters than the
        // limit permits.
        std::fs::write(
            forwards.join("mixed.url"),
            format!(
                "http://127.0.0.1:8766\"\\\u{1}\u{7f}<>\t{}\n",
                "A".repeat(300)
            ),
        )
        .unwrap();

        let report = scratch_report(home.path()).await;
        let url = entry(&report, "forwards", "mixed")["url"]
            .as_str()
            .unwrap()
            .to_string();
        // Exact, not "it parsed": seven neutralized characters, none of them
        // dropped, and the tail cut at the cap.
        let head = format!("http://127.0.0.1:8766{}", "?".repeat(7));
        assert_eq!(
            url,
            format!("{head}{}", "A".repeat(MAX_FIELD_CHARS - head.len()))
        );
        assert_eq!(url.chars().count(), MAX_FIELD_CHARS);
        assert_eq!(report["sanitizer_state"], SANITIZER_OK);
    }

    /// The regression. A value the host had must never arrive as a blank
    /// field, whatever it was made of.
    #[tokio::test]
    async fn script_never_reports_a_value_the_host_had_as_an_empty_field() {
        let home = tempfile::tempdir().unwrap();
        let (bin, forwards) = scratch_tree(home.path());
        write_fake_stado(
            &bin.join("stado"),
            "#!/bin/bash\nif [ \"$1\" = --version ]; then echo 'stado 9.9.9-test'; fi\n",
        );
        // Nothing in this marker is allowlisted, so nothing survives
        // sanitizing — and the field still may not come back empty.
        std::fs::write(forwards.join("opaque.url"), "\u{1}\u{2}<>\n").unwrap();
        std::fs::write(forwards.join("plain.url"), "http://127.0.0.1:8766\n").unwrap();
        write_vault(home.path(), "weles.vault.json", "{}\n", 0o600);

        let report = scratch_report(home.path()).await;
        assert_eq!(report["sanitizer_state"], SANITIZER_OK);
        assert_eq!(entry(&report, "forwards", "opaque")["url"], "????");
        assert_eq!(
            entry(&report, "forwards", "plain")["url"],
            "http://127.0.0.1:8766"
        );
        assert_eq!(report["managed_binaries"][0]["version"], "stado 9.9.9-test");

        // And the sweep the old code would have failed everywhere at once.
        for marker in report["forwards"].as_array().unwrap() {
            assert!(!marker["name"].as_str().unwrap().is_empty(), "{marker}");
            if marker["state"] == MARKER_READ {
                assert!(!marker["url"].as_str().unwrap().is_empty(), "{marker}");
            }
        }
        for binary in report["managed_binaries"].as_array().unwrap() {
            assert!(!binary["name"].as_str().unwrap().is_empty(), "{binary}");
            if binary["version_state"] == "reported" {
                assert!(!binary["version"].as_str().unwrap().is_empty(), "{binary}");
            }
        }
        for subcommand in report["subcommands"].as_array().unwrap() {
            assert!(
                !subcommand["name"].as_str().unwrap().is_empty(),
                "{subcommand}"
            );
        }
        for vault in report["vaults"].as_array().unwrap() {
            assert!(!vault["name"].as_str().unwrap().is_empty(), "{vault}");
            assert!(!vault["mode"].as_str().unwrap().is_empty(), "{vault}");
        }
    }

    /// The mini's own symptom in miniature: seven readable markers reported
    /// as `unreadable: 7`, because a blanked URL has no port to reconcile. A
    /// marker that went through the sanitizer must still be judged.
    #[tokio::test]
    async fn every_readable_marker_is_judged_and_never_counted_unreadable() {
        let home = tempfile::tempdir().unwrap();
        let (_bin, forwards) = scratch_tree(home.path());
        let live = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let live_port = live.local_addr().unwrap().port();
        let dead_port = {
            let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            socket.local_addr().unwrap().port()
        };
        for index in 0..5 {
            std::fs::write(
                forwards.join(format!("live-{index}.url")),
                format!("http://127.0.0.1:{live_port}\n"),
            )
            .unwrap();
        }
        for index in 0..2 {
            std::fs::write(
                forwards.join(format!("dead-{index}.url")),
                format!("http://127.0.0.1:{dead_port}\n"),
            )
            .unwrap();
        }

        let report = scratch_report(home.path()).await;
        assert_eq!(report["reconciliation"]["markers"], 7);
        assert_eq!(report["reconciliation"]["unreadable"], 0);
        for marker in report["forwards"].as_array().unwrap() {
            assert_eq!(marker["state"], MARKER_READ, "{marker}");
            assert_ne!(marker["port"], Value::Null, "{marker}");
            assert_ne!(marker["reconciliation"], UNREADABLE, "{marker}");
        }
        if !std::path::Path::new("/usr/sbin/netstat").exists() {
            // Off the macOS fleet this targets there is no socket table, so
            // no marker can be judged — which is `unknown`, still not
            // `unreadable`.
            assert_eq!(report["listeners_state"], LISTENERS_FAILED);
            assert_eq!(report["reconciliation"]["unknown"], 7);
            return;
        }
        assert_eq!(report["listeners_state"], LISTENERS_READ);
        assert_eq!(report["reconciliation"]["matched"], 5);
        assert_eq!(report["reconciliation"]["stale"], 2);
        assert_eq!(report["reconciliation"]["unknown"], 0);
        drop(live);
    }

    /// The failing condition itself: no external program the script names can
    /// run. The sanitizer needs none of them, so every name, mode and URL is
    /// still right; every fact that genuinely needed one is an explicit state
    /// instead of a blank.
    #[tokio::test]
    async fn report_is_still_named_when_no_external_program_can_run() {
        let home = tempfile::tempdir().unwrap();
        let (_bin, forwards) = scratch_tree(home.path());
        std::fs::write(
            forwards.join("stado-weles-api.url"),
            "http://127.0.0.1:8766\n",
        )
        .unwrap();
        write_vault(home.path(), "weles.vault.json", "{}\n", 0o600);

        let report = inventory_target(
            &target(),
            None,
            &prefixed_home_runner(home.path().to_path_buf(), NO_EXTERNAL_PROGRAMS),
        )
        .await
        .unwrap();

        assert_eq!(report["status"], OK_STATUS);
        assert_eq!(report["sanitizer_state"], SANITIZER_OK);
        let marker = entry(&report, "forwards", "stado-weles-api");
        assert_eq!(marker["state"], MARKER_READ);
        assert_eq!(marker["url"], "http://127.0.0.1:8766");
        assert_eq!(marker["port"], 8766);
        let vault = entry(&report, "vaults", "weles.vault.json");
        assert_eq!(vault["state"], VAULT_REGULAR);

        // Nothing above needed a process. Everything below did, and says so.
        assert_eq!(report["listeners_state"], LISTENERS_FAILED);
        assert_eq!(report["listeners"], json!([]));
        assert_eq!(marker["reconciliation"], UNKNOWN);
        assert_eq!(report["reconciliation"]["unknown"], 1);
        assert_eq!(report["reconciliation"]["stale"], 0);
        assert_eq!(vault["mode"], "unknown");
        assert_eq!(vault["bytes"], 0);
    }

    /// `skarbiec version` answers with a JSON object, not a line. Taking line
    /// one reported `{`, which the sanitizer correctly reduced to `?`:
    /// faultless sanitizing of the wrong value.
    #[tokio::test]
    async fn script_reads_a_version_out_of_a_json_answer() {
        let home = tempfile::tempdir().unwrap();
        let (bin, _forwards) = scratch_tree(home.path());
        write_fake_stado(
            &bin.join("skarbiec"),
            "#!/bin/bash\necho '{'\necho '  \"commit\": null,'\n\
             echo '  \"provenance\": \"source build\",'\necho '  \"release\": null,'\n\
             echo '  \"version\": \"0.1.3\"'\necho '}'\n",
        );
        let report = scratch_report(home.path()).await;
        assert_eq!(report["managed_binaries"][1]["version_state"], "reported");
        assert_eq!(report["managed_binaries"][1]["version"], "0.1.3");
    }

    /// A JSON answer with no version in it is not a version. Reporting a
    /// brace, or the next quoted member that happens to follow, would be
    /// worse than saying the answer could not be read.
    #[tokio::test]
    async fn json_answer_without_a_version_is_reported_unparsable() {
        let home = tempfile::tempdir().unwrap();
        let (bin, _forwards) = scratch_tree(home.path());
        write_fake_stado(
            &bin.join("skarbiec"),
            "#!/bin/bash\necho '{'\necho '  \"version\": null,'\n\
             echo '  \"release\": \"r7\"'\necho '}'\n",
        );
        let report = scratch_report(home.path()).await;
        assert_eq!(
            report["managed_binaries"][1]["version_state"],
            "version_unparsable"
        );
        assert_eq!(report["managed_binaries"][1]["version"], "");
    }
}
