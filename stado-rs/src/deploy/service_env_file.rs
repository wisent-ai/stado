//! Read the owner-controlled env FILE a managed unit sources, and reconcile
//! the endpoints it declares against what is actually listening on the host.
//!
//! NO Python original. This module exists because of a real outage on
//! 2026-08-30. `com.wisent.always-on.weles` on charless-mac-mini crash-looped
//! with `Skarbiec at http://127.0.0.1:8785 is unreachable`; the host's
//! Skarbiec was listening on 8895; `stado service env-set` had been used twice
//! to write the right port into `$HOME/.config/weles/worker.env`, and the unit
//! kept naming the wrong one. Nothing in Stado could read that file back.
//!
//! `stado service env` already answers "what environment does the UNIT FILE
//! declare" — it parses the plist / systemd unit. That is a different file
//! from the one a launcher `.`-sources, and on this fleet the interesting
//! values live in the sourced file, not in the unit. So the fleet had a
//! configuration surface it could WRITE ([`super::service::set_env_key_on_host`])
//! and could not READ, and `stado host exec` is an exact allowlist of
//! argument-free read-only programs with no file read in it at all. An
//! operator could change that file and never see the result. That is the gap
//! this module closes.
//!
//! Three properties are deliberate, because each one is a way the previous
//! state of the world lied:
//!
//! 1. **Duplicates are reported, in file order, with the winner named.** A
//!    sourced file assigns top to bottom, so a later `KEY=` silently wins and
//!    an earlier one is dead text. `set_env_key_on_host` strips lines matching
//!    `^KEY=` and appends the new assignment, which means it cannot see an
//!    `export KEY=…` spelling of the same variable at all. A reader that
//!    collapsed the file into a map would report the value the operator wanted
//!    and hide the reason the host disagrees.
//! 2. **Redaction happens ON THE HOST.** A value this command will not show is
//!    never put on the wire, never reaches the control-plane process, and
//!    never lands in a shell history or a JSON report. Only its length
//!    crosses. A hash prefix is deliberately NOT reported: a low-entropy
//!    secret is recoverable from one, and a length is not.
//! 3. **Endpoints are shown even when the key name looks like a credential.**
//!    `WELES_CREDENTIAL_SKARBIEC_URL` is the variable this outage turned on.
//!    A name-only redaction rule hides exactly the field an operator has to
//!    verify, so the rule here is value-shaped: an inert endpoint is shown
//!    whatever the key is called, and a URL carrying userinfo
//!    (`postgres://user:pass@host/db`) is redacted whatever the key is called.
//!
//! The transport is [`host_channel::run_script`] — the same approved encrypted
//! channel `env-set`, `file-sync` and `grant-sync` already use, with the same
//! `$HOME`-confinement prelude. `host exec`'s allowlist is untouched: the
//! listener read below runs `lsof` with the same fixed flags as the
//! already-approved `lsof -nP -iTCP -sTCP:LISTEN` entry, so the two readers
//! cannot disagree about what "listening" means, and no new free-form
//! capability is introduced.

use std::collections::BTreeMap;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::{host_channel, DeployError, Runner};
use crate::targets::ComputeTarget;

/// `status` for a report that came back whole.
pub const OK_STATUS: &str = "env_file";

/// The file was a regular file the login user could read.
pub const FILE_READ: &str = "read";
/// The path resolved outside the target's home and was never opened.
pub const FILE_REFUSED_OUTSIDE_HOME: &str = "refused_outside_home";
/// The path is a symlink. Never followed: a symlink under a home directory is
/// how a read of `~/.config/x.env` becomes a read of `~/.ssh/id_ed25519`.
pub const FILE_REFUSED_SYMLINK: &str = "refused_symlink";
/// There is no regular file at the path.
pub const FILE_MISSING: &str = "missing";
/// The file exists and the login user cannot read it.
pub const FILE_UNREADABLE: &str = "unreadable";

/// The file was parsed into entries.
pub const ENTRIES_READ: &str = "read";
/// The file was readable and the parser did not finish. An empty `entries`
/// means two opposite things depending on this field, exactly as every
/// `*_state` word in [`super::host_inventory`] does: "this file declares
/// nothing" and "nobody could tell" must not look identical.
pub const ENTRIES_PARSE_FAILED: &str = "parse_failed";
/// The file was never opened, so there was nothing to parse.
pub const ENTRIES_UNREAD: &str = "unread";

/// Listeners came from `lsof`: every row carries the owning program's name.
pub const LISTENERS_READ: &str = "read";
/// Listeners came from `netstat`, which names no owner. The ports are true
/// and the `process` column is empty for a stated reason.
pub const LISTENERS_READ_WITHOUT_NAMES: &str = "read_without_names";
/// Neither reader answered. No endpoint below could be reconciled.
pub const LISTENERS_FAILED: &str = "failed";

/// A value shown verbatim, as written in the file.
pub const VALUE_SHOWN: &str = "shown";
/// A value withheld. Only its length crossed the channel.
pub const VALUE_REDACTED: &str = "redacted";
/// A value shown because the operator named this exact key with `--reveal`.
pub const VALUE_REVEALED: &str = "revealed";
/// The assignment is present and its value is the empty string.
pub const VALUE_EMPTY: &str = "empty";

/// A plain `KEY=value` assignment.
pub const FORM_ASSIGNMENT: &str = "assignment";
/// An `export KEY=value` assignment. Assigns exactly like the plain form when
/// the file is sourced, and is invisible to `env-set`'s `^KEY=` rewrite —
/// which is why the two forms are reported as what they are instead of being
/// normalized into one.
pub const FORM_EXPORT: &str = "export";
/// A non-empty, non-comment line that is not an assignment at all: `. other.env`,
/// `set -a`, a stray word. Reported because a line like that changes what the
/// whole file means.
pub const FORM_UNPARSABLE: &str = "unparsable";

/// This assignment is the one a shell that sources the file ends up with.
pub const EFFECTIVE: &str = "effective";
/// A later assignment to the same key overwrites this one. Dead text.
pub const SHADOWED: &str = "shadowed";

/// Something is listening on the declared loopback port.
pub const ENDPOINT_LISTENING: &str = "listening";
/// Nothing is listening on the declared loopback port, and the socket table
/// was actually read. This is the verdict that fails the command.
pub const ENDPOINT_DEAD: &str = "dead";
/// The socket table could not be read, so the declaration was not judged.
pub const ENDPOINT_UNKNOWN: &str = "unknown";
/// The endpoint is not loopback. This host's socket table cannot answer for it.
pub const ENDPOINT_REMOTE: &str = "remote";

/// The cap on how many characters of one reported value cross the channel.
/// Larger than [`super::host_inventory::MAX_FIELD_CHARS`] because a database
/// URL or an allowlist is legitimately long, and a truncated endpoint is
/// useless for the one job this command has.
pub const MAX_VALUE_CHARS: usize = 400;

/// The cap on how many assignments are reported. A file past this is reported
/// as truncated through `entries_seen`, never silently cut.
pub const MAX_ENTRIES: usize = 400;

/// One line of the env file that assigns something, or fails to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvEntry {
    /// 1-based line number in the file, so the operator can go to the line.
    pub line: u32,
    /// [`FORM_ASSIGNMENT`], [`FORM_EXPORT`] or [`FORM_UNPARSABLE`].
    pub form: String,
    /// The variable name, empty for [`FORM_UNPARSABLE`].
    pub key: String,
    /// [`VALUE_SHOWN`], [`VALUE_REDACTED`], [`VALUE_REVEALED`] or
    /// [`VALUE_EMPTY`].
    pub value_state: String,
    /// The text after `=`, exactly as written (quotes included), sanitized to
    /// printable ASCII. Empty whenever `value_state` is [`VALUE_REDACTED`],
    /// and the state beside it says so.
    pub value: String,
    /// How many characters the value has, quotes removed. Reported for a
    /// redacted value too: "the token is 0 characters long" is a finding.
    pub chars: u32,
}

/// One listening TCP socket, and the program holding it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcListener {
    /// `127.0.0.1`, `::1`, or `*` for a socket bound to every interface,
    /// which answers on loopback too.
    pub address: String,
    pub port: u32,
    pub pid: u32,
    /// The program name `lsof` reported, empty under
    /// [`LISTENERS_READ_WITHOUT_NAMES`]. `lsof` truncates this to nine
    /// characters; the flags are held identical to the already-approved
    /// `host exec` entry rather than widened, so both readers report the same
    /// name for the same process.
    pub process: String,
}

/// Everything the remote script reported about one env file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvFileReport {
    /// The absolute path the host resolved, empty when it refused to resolve one.
    pub path: String,
    /// [`FILE_READ`] or one of the refusals above.
    pub file_state: String,
    /// Why the file state is what it is, in the host's own words.
    pub detail: String,
    /// Permission bits in octal (`600`), or `unknown`.
    pub mode: String,
    /// No group and no other bits. An env file the group can read is a
    /// finding, not a cosmetic detail.
    pub owner_only: bool,
    pub bytes: u64,
    /// [`ENTRIES_READ`], [`ENTRIES_PARSE_FAILED`] or [`ENTRIES_UNREAD`].
    pub entries_state: String,
    pub entries: Vec<EnvEntry>,
    /// How many assignments the file has, including any past [`MAX_ENTRIES`]
    /// that `entries` therefore does not list.
    pub entries_seen: u32,
    /// [`LISTENERS_READ`], [`LISTENERS_READ_WITHOUT_NAMES`] or
    /// [`LISTENERS_FAILED`].
    pub listeners_state: String,
    pub listeners: Vec<ProcListener>,
}

/// A loopback endpoint one env value declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Endpoint {
    pub port: u32,
    /// Whether the authority names this machine. Only a loopback endpoint can
    /// be reconciled against this host's own socket table.
    pub loopback: bool,
}

/// The remote program.
///
/// One `awk` pass does the parsing, the classification and the JSON, and it is
/// the only fork in the file half of this script. The shell-builtin sanitizer
/// [`super::host_inventory`] uses is right for a hundred short names and wrong
/// here: an env file is hundreds of long values, and a per-character shell loop
/// over all of them is quadratic work for no gain. An `awk` that dies is
/// reported as [`ENTRIES_PARSE_FAILED`] rather than as a file with nothing in
/// it, which is the failure mode that matters.
///
/// The classification lives in `awk`, on the host, because that is what makes
/// "a redacted value never crosses the channel" true rather than merely
/// intended.
const REMOTE_ENV_FILE_BODY: &str = r##"set -eu
LC_ALL=C
export LC_ALL

home=$HOME
decode=-D
if [ "$(uname)" = "Linux" ]; then decode=--decode; fi
env_path=$(printf '%s' '@ENV_PATH_B64@' | /usr/bin/base64 "$decode")
reveal=$(printf '%s' '@REVEAL_B64@' | /usr/bin/base64 "$decode")

# Every field below is either a compile-time constant of this script or has
# been through the awk sanitizer, so `report` never carries host text that
# could break the payload.
report() {
  printf '{"path":"%s","file_state":"%s","detail":"%s","mode":"%s","owner_only":%s,"bytes":%s,"entries_state":"%s",%s,"listeners_state":"%s","listeners":[%s]}\n' \
    "$1" "$2" "$3" "$4" "$5" "$6" "$7" "$8" "$9" "${10}"
}

# A refusal is a complete report with an explicit state, not an error exit:
# the caller has to be able to tell "this path is a symlink" from "the channel
# broke", and an empty entries list has to arrive with the reason beside it.
refuse() {
  report '' "$1" "$2" unknown false 0 unread '"entries":[],"entries_seen":0' unread ''
  exit 0
}

# The $HOME-confinement prelude of `service.rs::set_env_key_on_host`, word for
# word. The command that READS a managed env file must accept exactly the paths
# the command that WRITES one accepts; a reader with a wider rule would be a
# file-read primitive wearing an env-file's name.
case "$env_path" in
  '$HOME'/*) env_path="$home/${env_path#\$HOME/}" ;;
  "$home"/*) ;;
  /*) refuse refused_outside_home 'the target must be inside the target home' ;;
  *) env_path="$home/$env_path" ;;
esac
case "$env_path" in "$home"/*) ;; *) refuse refused_outside_home 'the target must be inside the target home' ;; esac
# -L before -f, never the other way round: -f follows the link, so a symlink
# would be reported as a present file and its target read instead.
if [ -L "$env_path" ]; then
  refuse refused_symlink 'the target is a symlink and was not followed'
fi
if [ ! -f "$env_path" ]; then
  refuse missing 'no regular file at the target'
fi
parent=$(/usr/bin/dirname "$env_path")
real_parent=$(/usr/bin/python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$parent")
if ! /usr/bin/python3 -c 'import os,sys; home=os.path.realpath(sys.argv[1]); parent=sys.argv[2]; sys.exit(0 if os.path.commonpath((home,parent)) == home else 1)' "$home" "$real_parent"; then
  refuse refused_outside_home 'the resolved target leaves the target home'
fi
if [ ! -r "$env_path" ]; then
  refuse unreadable 'the login user cannot read the target'
fi

# Both stat(1) dialects, BSD first, exactly as the vault section of
# host_inventory.rs probes them. Neither opens the file.
if facts=$(/usr/bin/stat -f '%z %Lp' "$env_path" 2>/dev/null); then
  :
elif facts=$(/usr/bin/stat -c '%s %a' "$env_path" 2>/dev/null); then
  :
else
  facts=""
fi
bytes=${facts%% *}
mode=${facts#* }
case "$bytes" in
  ''|*[!0-9]*) bytes=0 ;;
esac
if [ "$mode" = "$facts" ]; then
  mode=unknown
fi
case "$mode" in
  0???) mode=${mode#0} ;;
esac
case "$mode" in
  *[!0-7]*) mode=unknown ;;
esac
case "$mode" in
  *00) owner_only=true ;;
  *) owner_only=false ;;
esac

entries_state=read
if ! entries_fragment=$(/usr/bin/awk \
    -v reveal="$reveal" \
    -v max_entries=@MAX_ENTRIES@ \
    -v max_chars=@MAX_VALUE_CHARS@ '
# Printable ASCII only, minus the two bytes a JSON string cannot carry raw.
# Refusing them outright rather than escaping them is a guarantee that does
# not depend on getting the escaping right, and a corrupt or hostile file
# cannot emit a quote, a backslash, a newline or a control character into this
# report.
function jsonsafe(text) {
  gsub(/[^ -~]/, "?", text)
  gsub(/["\\]/, "?", text)
  return text
}
function clamp(text) {
  if (length(text) > max_chars) {
    return substr(text, 1, max_chars - 3) "..."
  }
  return text
}
# One layer of matching surrounding quotes, the way a shell would remove it.
# Only for the classification and the length: the reported text keeps its
# quotes, because whether a value is quoted is part of what the operator is
# reading the file to find out.
function unquote(text,   size, head, tail) {
  size = length(text)
  if (size >= 2) {
    head = substr(text, 1, 1)
    tail = substr(text, size, 1)
    if ((head == "\"" && tail == "\"") || (head == "'"'"'" && tail == "'"'"'")) {
      return substr(text, 2, size - 2)
    }
  }
  return text
}
# scheme://userinfo@host — a URL that carries a credential in its authority.
# Redacted whatever the key is called: DATABASE_URL names no secret and holds one.
function has_userinfo(text) {
  return (text ~ /^[A-Za-z][A-Za-z0-9+.-]*:\/\/[^\/@]*@/)
}
# An endpoint, a port, a plain flag, or a reference to another variable. Every
# one of these is a value an operator has to be able to read, and none of them
# can carry a credential: the URL form admits no @, ? or # by construction.
function inert(text) {
  if (text ~ /^\$\{?[A-Za-z_][A-Za-z0-9_]*\}?$/) return 1
  if (text ~ /^[0-9]+$/) return 1
  if (text ~ /^(true|false|yes|no|on|off|TRUE|FALSE|YES|NO|ON|OFF)$/) return 1
  if (text ~ /^[A-Za-z][A-Za-z0-9+.-]*:\/\/[A-Za-z0-9._-]+(:[0-9]+)?(\/[A-Za-z0-9._~\/-]*)?$/) return 1
  if (text ~ /^[A-Za-z0-9._-]+:[0-9]+$/) return 1
  return 0
}
function secretish(name) {
  return (name ~ /TOKEN|SECRET|PASSWORD|PASSWD|PASSPHRASE|CREDENTIAL|KEY|BEARER|PRIVATE|SIGNING|SIGNATURE|SALT|COOKIE|AUTH|SESSION/)
}
{
  line = $0
  sub(/\r$/, "", line)
  sub(/^[ \t]+/, "", line)
  if (line == "" || line ~ /^#/) next
  seen++
  if (seen > max_entries) next
  form = "assignment"
  if (line ~ /^export[ \t]+/) {
    form = "export"
    sub(/^export[ \t]+/, "", line)
  }
  key = ""
  value = ""
  if (match(line, /^[A-Za-z_][A-Za-z0-9_]*=/)) {
    key = substr(line, 1, RLENGTH - 1)
    value = substr(line, RLENGTH + 1)
    sub(/[ \t]+$/, "", value)
  } else {
    form = "unparsable"
  }
  if (form == "unparsable") {
    # Shown, because `. other.env` or `set -a` changes what the whole file
    # means and hiding it is what made this class of fault unreadable. A line
    # that MENTIONS a credential is withheld anyway.
    chars = length(line)
    if (secretish(line)) {
      state = "redacted"
      shown = ""
    } else {
      state = "shown"
      shown = line
    }
  } else {
    probe = unquote(value)
    chars = length(probe)
    if (probe == "") {
      state = "empty"
      shown = ""
    } else if (key == reveal) {
      state = "revealed"
      shown = value
    } else if (has_userinfo(probe)) {
      state = "redacted"
      shown = ""
    } else if (inert(probe)) {
      state = "shown"
      shown = value
    } else if (secretish(key)) {
      state = "redacted"
      shown = ""
    } else {
      state = "shown"
      shown = value
    }
  }
  out = out sep sprintf("{\"line\":%d,\"form\":\"%s\",\"key\":\"%s\",\"value_state\":\"%s\",\"value\":\"%s\",\"chars\":%d}", \
    NR, form, jsonsafe(key), state, clamp(jsonsafe(shown)), chars)
  sep = ","
}
END {
  printf "\"entries\":[%s],\"entries_seen\":%d", out, seen + 0
}
' "$env_path"); then
  entries_state=parse_failed
  entries_fragment='"entries":[],"entries_seen":0'
fi

# The socket table, with the program that owns each port. lsof first, with the
# same fixed flags as the approved `host exec` entry so the two readers cannot
# disagree; netstat second, which answers the port question and not the owner
# question, and says so through its own state.
listeners_state=failed
listeners_json=""
listener_source=""
raw=""
# Judged on whether the reader produced a table, not on its exit status: lsof
# routinely exits non-zero after listing every socket it could see, because
# one file descriptor somewhere refused to be identified. Treating that as
# "the socket table could not be read" would report every endpoint on a
# healthy host as unjudged.
for candidate in /usr/sbin/lsof /usr/bin/lsof; do
  if [ -x "$candidate" ]; then
    raw=$("$candidate" -nP -iTCP -sTCP:LISTEN 2>/dev/null) || raw=""
    if [ -n "$raw" ]; then
      listener_source=lsof
      break
    fi
  fi
done
if [ -z "$listener_source" ]; then
  for candidate in /usr/sbin/netstat /bin/netstat /usr/bin/netstat; do
    if [ -x "$candidate" ]; then
      raw=$("$candidate" -anv -p tcp 2>/dev/null) || raw=""
      if [ -n "$raw" ]; then
        listener_source=netstat
        break
      fi
    fi
  done
fi
if [ "$listener_source" = lsof ]; then
  if listeners_json=$(printf '%s\n' "$raw" | /usr/bin/awk '
function jsonsafe(text) {
  gsub(/[^ -~]/, "?", text)
  gsub(/["\\]/, "?", text)
  return text
}
NR == 1 { next }
$NF == "(LISTEN)" {
  address = $(NF - 1)
  if (!match(address, /:[0-9]+$/)) next
  port = substr(address, RSTART + 1) + 0
  authority = substr(address, 1, RSTART - 1)
  if (authority != "*" && authority != "::1" && authority != "[::1]" && authority !~ /^127\./) next
  if (seen[port "/" $2]++) next
  printf "%s{\"address\":\"%s\",\"port\":%d,\"pid\":%d,\"process\":\"%s\"}", \
    (emitted++ ? "," : ""), jsonsafe(authority), port, $2 + 0, jsonsafe($1)
}
'); then
    listeners_state=read
  else
    listeners_json=""
  fi
elif [ "$listener_source" = netstat ]; then
  if listeners_json=$(printf '%s\n' "$raw" | /usr/bin/awk '
$6 != "LISTEN" { next }
{
  address = $4
  parts_count = split(address, parts, ".")
  port = parts[parts_count]
  if (port !~ /^[0-9]+$/) next
  authority = substr(address, 1, length(address) - length(port) - 1)
  if (authority != "*" && authority != "::1" && authority !~ /^127\./) next
  if (seen[port]++) next
  pid = 0
  for (field = 7; field <= NF; field++) {
    if ($field ~ /:[0-9]+$/) {
      pid = substr($field, index($field, ":") + 1)
      break
    }
  }
  printf "%s{\"address\":\"%s\",\"port\":%d,\"pid\":%d,\"process\":\"\"}", \
    (emitted++ ? "," : ""), authority, port + 0, pid + 0
}
'); then
    listeners_state=read_without_names
  else
    listeners_json=""
  fi
fi

report "$(printf '%s' "$env_path" | /usr/bin/awk '{ gsub(/[^ -~]/, "?"); gsub(/["\\]/, "?"); printf "%s", $0 }')" \
  read '' "$mode" "$owner_only" "$bytes" "$entries_state" "$entries_fragment" \
  "$listeners_state" "$listeners_json"
"##;

/// The remote program for one env file, with the reveal selection bound in.
///
/// Both operands travel base64-encoded inside the request body, never in an
/// argument vector, for the same reason `env-set` encodes its value: the
/// script text is the only thing that reaches the host.
pub fn remote_env_file_script(env_path: &str, reveal: Option<&str>) -> String {
    REMOTE_ENV_FILE_BODY
        .replace("@ENV_PATH_B64@", &STANDARD.encode(env_path.as_bytes()))
        .replace(
            "@REVEAL_B64@",
            &STANDARD.encode(reveal.unwrap_or_default().as_bytes()),
        )
        .replace("@MAX_ENTRIES@", &MAX_ENTRIES.to_string())
        .replace("@MAX_VALUE_CHARS@", &MAX_VALUE_CHARS.to_string())
}

/// Parse the script's one line of JSON.
///
/// The LAST line starting with `{` is the payload, for the reason
/// [`super::host_inventory::parse_inventory`] gives: a login shell that greets
/// its callers must not turn a healthy host into a parse error.
pub fn parse_env_file(stdout: &str) -> Result<EnvFileReport, DeployError> {
    let payload = stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with('{'))
        .ok_or_else(|| DeployError("env file script produced no JSON report".to_string()))?;
    let mut report: EnvFileReport = serde_json::from_str(payload).map_err(|error| {
        DeployError(format!(
            "env file script did not return the expected JSON: {error}"
        ))
    })?;
    report.entries_seen = report.entries_seen.max(report.entries.len() as u32);
    report.entries.truncate(MAX_ENTRIES);
    Ok(report)
}

/// The value a shell would end up with, quotes removed and a trailing
/// unquoted comment dropped.
///
/// Interpretation lives here rather than on the host so it can be tested
/// against the shapes real env files use. The host's copy of the unquoting
/// exists for one reason only — to decide what may be shown — and this one
/// decides what the value MEANS.
pub fn effective_text(value: &str) -> &str {
    let trimmed = value.trim();
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 2 {
        let head = bytes[usize::MIN];
        if (head == b'"' || head == b'\'') && bytes[bytes.len() - 1] == head {
            return &trimmed[1..trimmed.len() - 1];
        }
    }
    // `KEY=value # note` is a comment to every shell that sources the file,
    // and only for an unquoted value.
    match trimmed.split_once(" #") {
        Some((head, _)) => head.trim_end(),
        None => trimmed,
    }
}

/// Whether an authority names this machine.
fn authority_is_loopback(authority: &str) -> bool {
    matches!(
        authority,
        "127.0.0.1" | "localhost" | "::1" | "[::1]" | "0.0.0.0" | "*"
    )
}

/// The endpoint one assignment declares, or `None` for a value that is not an
/// endpoint at all.
///
/// A bare integer is only read as a port for a key that says it is one.
/// `WELES_MAX_CONCURRENCY=4` is not a declaration that something must be
/// listening on port 4, and reporting it as a dead dependency would make this
/// command's non-zero exit meaningless.
pub fn declared_endpoint(key: &str, value: &str) -> Option<Endpoint> {
    let text = effective_text(value);
    if text.is_empty() {
        return None;
    }
    if let Some((scheme, rest)) = text.split_once("://") {
        if scheme.is_empty()
            || !scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '.' || c == '-')
        {
            return None;
        }
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        // A URL carrying userinfo is never reported by the host, so reaching
        // this with one means an operator revealed it; the credential half is
        // not part of the endpoint either way.
        let authority = authority.rsplit('@').next().unwrap_or_default();
        let (host, port) = match authority.strip_prefix('[') {
            // `[::1]:8895` — the colon that separates the port is the one
            // after the closing bracket, not the ones inside it.
            Some(rest) => match rest.split_once("]:") {
                Some((host, port)) => (format!("[{host}]"), Some(port)),
                None => (authority.to_string(), None),
            },
            None => match authority.rsplit_once(':') {
                Some((host, port)) => (host.to_string(), Some(port)),
                None => (authority.to_string(), None),
            },
        };
        let port = match port {
            Some(port) => port.parse::<u32>().ok()?,
            None => match scheme {
                "http" | "ws" => 80,
                "https" | "wss" => 443,
                _ => return None,
            },
        };
        if port == u32::MIN || port > u32::from(u16::MAX) {
            return None;
        }
        return Some(Endpoint {
            port,
            loopback: authority_is_loopback(&host),
        });
    }
    if key == "PORT" || key.ends_with("_PORT") {
        let port = text.parse::<u32>().ok()?;
        if port == u32::MIN || port > u32::from(u16::MAX) {
            return None;
        }
        return Some(Endpoint {
            port,
            loopback: true,
        });
    }
    // `127.0.0.1:8895` with no scheme, the spelling a `*_HOST` or `*_ADDR`
    // variable usually carries.
    let (host, port) = text.rsplit_once(':')?;
    let port = port.parse::<u32>().ok()?;
    if port == u32::MIN
        || port > u32::from(u16::MAX)
        || host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        return None;
    }
    Some(Endpoint {
        port,
        loopback: authority_is_loopback(host),
    })
}

/// One entry's role among every assignment to the same key, in file order:
/// [`EFFECTIVE`] for the last one, [`SHADOWED`] for every earlier one.
///
/// This is the finding the whole command exists for. A sourced file assigns
/// top to bottom, so the LAST assignment is the one the process runs with, and
/// an operator reading a `KEY=` near the top of the file is reading dead text.
/// Both spellings count: `export KEY=…` assigns exactly like `KEY=…`, and
/// `env-set`'s `^KEY=` rewrite cannot see the export form at all — so a file
/// can hold an `export` line the writer will never replace.
pub fn shadowing(entries: &[EnvEntry]) -> Vec<&'static str> {
    let mut last: BTreeMap<&str, usize> = BTreeMap::new();
    for (index, entry) in entries.iter().enumerate() {
        if entry.form == FORM_UNPARSABLE || entry.key.is_empty() {
            continue;
        }
        last.insert(entry.key.as_str(), index);
    }
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            if entry.form == FORM_UNPARSABLE || entry.key.is_empty() {
                return "";
            }
            if last.get(entry.key.as_str()) == Some(&index) {
                EFFECTIVE
            } else {
                SHADOWED
            }
        })
        .collect()
}

/// Every key the file assigns more than once, in first-appearance order.
pub fn duplicate_keys(entries: &[EnvEntry]) -> Vec<String> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for entry in entries {
        if entry.form == FORM_UNPARSABLE || entry.key.is_empty() {
            continue;
        }
        *counts.entry(entry.key.as_str()).or_default() += 1;
    }
    let mut seen: Vec<String> = Vec::new();
    for entry in entries {
        if counts.get(entry.key.as_str()).copied().unwrap_or_default() > 1
            && !seen.iter().any(|key| key == &entry.key)
        {
            seen.push(entry.key.clone());
        }
    }
    seen
}

/// One declared endpoint's verdict against the socket table, and every
/// process holding that port.
///
/// `listeners_state` is not decoration, for the reason
/// [`super::host_inventory::verdict`] states: "nothing is listening" and
/// "nobody could ask" are opposite findings that look identical in an empty
/// list, and calling the second one dead reports a working host as broken.
pub fn endpoint_verdict(
    endpoint: Endpoint,
    listeners: &[ProcListener],
    listeners_state: &str,
) -> (&'static str, Vec<String>) {
    if !endpoint.loopback {
        return (ENDPOINT_REMOTE, Vec::new());
    }
    if listeners_state != LISTENERS_READ && listeners_state != LISTENERS_READ_WITHOUT_NAMES {
        return (ENDPOINT_UNKNOWN, Vec::new());
    }
    let holders: Vec<String> = listeners
        .iter()
        .filter(|listener| listener.port == endpoint.port)
        .map(|listener| {
            if listener.process.is_empty() {
                format!("pid {}", listener.pid)
            } else {
                format!("{} (pid {})", listener.process, listener.pid)
            }
        })
        .collect();
    if holders.is_empty() {
        (ENDPOINT_DEAD, holders)
    } else {
        (ENDPOINT_LISTENING, holders)
    }
}

/// One row of the endpoint reconciliation: a key, what it declares, and
/// whether anything answers there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointRow {
    pub key: String,
    pub line: u32,
    pub declared: String,
    pub port: u32,
    pub verdict: &'static str,
    pub holders: Vec<String>,
}

/// Reconcile every endpoint the file's EFFECTIVE assignments declare.
///
/// Only the effective assignment of each key is judged: a shadowed line is
/// not what the process runs with, and failing the command on a dead endpoint
/// nothing reads would be a false alarm. The shadowed lines are still reported
/// — by [`shadowing`], where they belong.
pub fn endpoint_rows(report: &EnvFileReport) -> Vec<EndpointRow> {
    let roles = shadowing(&report.entries);
    let mut rows = Vec::new();
    for (entry, role) in report.entries.iter().zip(roles) {
        if role != EFFECTIVE || entry.value_state == VALUE_REDACTED {
            continue;
        }
        let Some(endpoint) = declared_endpoint(&entry.key, &entry.value) else {
            continue;
        };
        let (verdict, holders) =
            endpoint_verdict(endpoint, &report.listeners, &report.listeners_state);
        rows.push(EndpointRow {
            key: entry.key.clone(),
            line: entry.line,
            declared: effective_text(&entry.value).to_string(),
            port: endpoint.port,
            verdict,
            holders,
        });
    }
    rows
}

/// The env file as the `--json` report, in `host inventory`'s report shape.
pub fn to_report(target: &ComputeTarget, unit: &str, report: &EnvFileReport) -> Map<String, Value> {
    let mut payload = host_channel::base_report(target);
    payload.insert("unit".to_string(), json!(unit));
    payload.insert("env_file".to_string(), json!(report.path));
    payload.insert("file_state".to_string(), json!(report.file_state));
    payload.insert("detail".to_string(), json!(report.detail));
    payload.insert("mode".to_string(), json!(report.mode));
    payload.insert("owner_only".to_string(), json!(report.owner_only));
    payload.insert("bytes".to_string(), json!(report.bytes));
    payload.insert("entries_state".to_string(), json!(report.entries_state));
    payload.insert("entries_seen".to_string(), json!(report.entries_seen));
    let roles = shadowing(&report.entries);
    payload.insert(
        "entries".to_string(),
        Value::Array(
            report
                .entries
                .iter()
                .zip(&roles)
                .map(|(entry, role)| {
                    json!({
                        "line": entry.line,
                        "form": entry.form,
                        "key": entry.key,
                        "value_state": entry.value_state,
                        "value": entry.value,
                        "chars": entry.chars,
                        "resolution": role,
                    })
                })
                .collect(),
        ),
    );
    payload.insert(
        "duplicate_keys".to_string(),
        json!(duplicate_keys(&report.entries)),
    );
    payload.insert(
        "redacted".to_string(),
        json!(report
            .entries
            .iter()
            .filter(|entry| entry.value_state == VALUE_REDACTED)
            .count()),
    );
    payload
}

/// Read one already-resolved registry host's env file.
///
/// Split out from the CLI for the reason
/// [`super::host_inventory::inventory_target`] is: the whole read is
/// exercisable through the [`Runner`] seam without a registry.
pub async fn read_env_file(
    target: &ComputeTarget,
    env_path: &str,
    reveal: Option<&str>,
    runner: &Runner,
) -> Result<EnvFileReport, DeployError> {
    let script = remote_env_file_script(env_path, reveal);
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(format!(
            "{}: {}",
            target.name,
            host_channel::last_error_line(&output, "ssh failed")
        )));
    }
    parse_env_file(&output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(line: u32, form: &str, key: &str, value: &str) -> EnvEntry {
        EnvEntry {
            line,
            form: form.to_string(),
            key: key.to_string(),
            value_state: VALUE_SHOWN.to_string(),
            value: value.to_string(),
            chars: value.len() as u32,
        }
    }

    #[test]
    fn the_last_assignment_wins_across_both_spellings() {
        let entries = vec![
            entry(
                1,
                FORM_ASSIGNMENT,
                "WC_SKARBIEC_URL",
                "http://127.0.0.1:8895",
            ),
            entry(2, FORM_ASSIGNMENT, "OTHER", "1"),
            entry(3, FORM_EXPORT, "WC_SKARBIEC_URL", "http://127.0.0.1:8785"),
        ];
        assert_eq!(shadowing(&entries), vec![SHADOWED, EFFECTIVE, EFFECTIVE]);
        assert_eq!(duplicate_keys(&entries), vec!["WC_SKARBIEC_URL"]);
    }

    #[test]
    fn an_unparsable_line_belongs_to_no_key() {
        let mut sourced = entry(4, FORM_UNPARSABLE, "", ". other.env");
        sourced.key = String::new();
        let entries = vec![entry(1, FORM_ASSIGNMENT, "A", "1"), sourced];
        assert_eq!(shadowing(&entries), vec![EFFECTIVE, ""]);
        assert!(duplicate_keys(&entries).is_empty());
    }

    #[test]
    fn quotes_and_trailing_comments_are_not_part_of_the_value() {
        assert_eq!(
            effective_text("\"http://127.0.0.1:8895\""),
            "http://127.0.0.1:8895"
        );
        assert_eq!(effective_text("'8895'"), "8895");
        assert_eq!(
            effective_text("http://127.0.0.1:8895 # live"),
            "http://127.0.0.1:8895"
        );
        assert_eq!(effective_text("  spaced  "), "spaced");
    }

    #[test]
    fn only_a_port_shaped_key_reads_a_bare_integer_as_a_port() {
        assert_eq!(
            declared_endpoint("WELES_API_PORT", "8896"),
            Some(Endpoint {
                port: 8896,
                loopback: true
            })
        );
        assert_eq!(declared_endpoint("WELES_MAX_CONCURRENCY", "4"), None);
    }

    #[test]
    fn loopback_and_remote_urls_are_told_apart() {
        assert_eq!(
            declared_endpoint("WC_SKARBIEC_URL", "http://127.0.0.1:8785"),
            Some(Endpoint {
                port: 8785,
                loopback: true
            })
        );
        assert_eq!(
            declared_endpoint("STADO_API_URL", "https://api.example.com/v1"),
            Some(Endpoint {
                port: 443,
                loopback: false
            })
        );
        assert_eq!(
            declared_endpoint("WELES_HOST", "127.0.0.1:18100"),
            Some(Endpoint {
                port: 18100,
                loopback: true
            })
        );
        assert_eq!(
            declared_endpoint("WELES_IPV6_URL", "http://[::1]:8765/"),
            Some(Endpoint {
                port: 8765,
                loopback: true
            })
        );
        assert_eq!(declared_endpoint("WELES_NOTE", "some prose"), None);
    }

    #[test]
    fn a_dead_endpoint_is_only_dead_when_the_socket_table_was_read() {
        let endpoint = Endpoint {
            port: 8785,
            loopback: true,
        };
        assert_eq!(
            endpoint_verdict(endpoint, &[], LISTENERS_FAILED).0,
            ENDPOINT_UNKNOWN
        );
        assert_eq!(
            endpoint_verdict(endpoint, &[], LISTENERS_READ).0,
            ENDPOINT_DEAD
        );
        let holder = ProcListener {
            address: "127.0.0.1".to_string(),
            port: 8785,
            pid: 4242,
            process: "skarbiec".to_string(),
        };
        let (verdict, holders) = endpoint_verdict(endpoint, &[holder], LISTENERS_READ);
        assert_eq!(verdict, ENDPOINT_LISTENING);
        assert_eq!(holders, vec!["skarbiec (pid 4242)"]);
    }

    #[test]
    fn a_shadowed_endpoint_is_never_the_one_judged() {
        let report = EnvFileReport {
            path: "/home/u/.config/weles/worker.env".to_string(),
            file_state: FILE_READ.to_string(),
            detail: String::new(),
            mode: "600".to_string(),
            owner_only: true,
            bytes: 64,
            entries_state: ENTRIES_READ.to_string(),
            entries: vec![
                entry(
                    1,
                    FORM_ASSIGNMENT,
                    "WC_SKARBIEC_URL",
                    "http://127.0.0.1:8895",
                ),
                entry(2, FORM_EXPORT, "WC_SKARBIEC_URL", "http://127.0.0.1:8785"),
            ],
            entries_seen: 2,
            listeners_state: LISTENERS_READ.to_string(),
            listeners: vec![ProcListener {
                address: "127.0.0.1".to_string(),
                port: 8895,
                pid: 31909,
                process: "skarbiec".to_string(),
            }],
        };
        let rows = endpoint_rows(&report);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[usize::MIN].port, 8785);
        assert_eq!(rows[usize::MIN].verdict, ENDPOINT_DEAD);
    }
}
