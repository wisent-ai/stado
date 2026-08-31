//! Fetch one file out of a managed host's home, byte-exact and
//! integrity-checked end to end.
//!
//! NO Python original. This module exists because of what
//! [`super::service_env_file`] deliberately cannot do. That reader sanitizes
//! every value it reports — printable ASCII only, quotes and backslashes
//! replaced with `?`, long values clamped — because its job is to let an
//! operator *judge* a configuration file without a secret ever crossing the
//! channel. The consequence is that it can diagnose a file and can never
//! reproduce one byte of it.
//!
//! That gap has a name on this fleet. `$HOME/.stado/bin/weles-release-cutover`
//! on charless-mac-mini is 4357 bytes of live operator tooling that is checked
//! into no repository: it rewrote `$HOME/.config/weles/worker.env` on every
//! launchd restart for days, and the only copy of the code doing it was on the
//! host. `stado host exec` is an allowlist of argument-free read-only programs
//! with no file read in it. `service file-sync` moves a file the other way.
//! `service env-show` would have returned a redacted paraphrase. So the only
//! way to put that script under version control was to copy it off the box by
//! hand, outside the approved channel — which is the one thing the fleet-wide
//! "everything through Stado" rule exists to prevent.
//!
//! Three properties are deliberate:
//!
//! 1. **The digest is computed on the host and re-computed here, over the
//!    decoded bytes.** A base64 payload that lost a chunk in a login banner, a
//!    truncating channel, or a `stdout` cap decodes into something shorter and
//!    perfectly valid, so length alone proves nothing. The comparison is of two
//!    independently computed SHA-256s of the same bytes at the two ends of the
//!    channel, and a mismatch is [`INTEGRITY_MISMATCH`] with nothing written.
//! 2. **The confinement is [`super::service_env_file`]'s, word for word.** The
//!    command that copies a managed file must accept exactly the paths the
//!    commands that read and write one accept. A fetch with a wider rule would
//!    be an arbitrary remote-read primitive wearing a service verb's name, and
//!    `~/.config/x.env` resolving through a symlink into `~/.ssh/id_ed25519` is
//!    the exact shape that makes it one. `-L` is tested before `-f`, because
//!    `-f` follows the link.
//! 3. **A refusal is a complete report, not an error exit.** "this path is a
//!    symlink", "there is no file there" and "the channel broke" are three
//!    different findings, and a caller that cannot tell them apart cannot act
//!    on any of them.
//!
//! The transport is [`host_channel::run_script`] — the same approved encrypted
//! channel `env-show`, `env-set`, `file-sync` and `grant-sync` use, with the
//! operand carried base64-encoded inside the request body and never in an
//! argument vector.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::{host_channel, DeployError, Runner};
use crate::targets::ComputeTarget;

/// `status` for a report that came back whole.
pub const OK_STATUS: &str = "file_fetch";

/// The file was a regular file the login user could read, and its bytes came
/// back.
pub const FILE_READ: &str = "read";
/// The path resolved outside the target's home and was never opened.
pub const FILE_REFUSED_OUTSIDE_HOME: &str = "refused_outside_home";
/// The path is a symlink. Never followed, for [`super::service_env_file`]'s
/// reason: a symlink under a home directory is how a read of `~/.config/x.env`
/// becomes a read of `~/.ssh/id_ed25519`.
pub const FILE_REFUSED_SYMLINK: &str = "refused_symlink";
/// There is no regular file at the path.
pub const FILE_MISSING: &str = "missing";
/// The file exists and the login user cannot read it.
pub const FILE_UNREADABLE: &str = "unreadable";
/// The file is larger than [`MAX_FETCH_BYTES`] and was never read. A fetch
/// that silently returned a prefix would be the worst possible answer here:
/// the digest would match the prefix and the caller would commit a truncated
/// program.
pub const FILE_REFUSED_TOO_LARGE: &str = "refused_too_large";
/// The host has neither SHA-256 tool, so no digest could be computed and
/// nothing was transferred. A fetch with no digest is not a fetch this command
/// performs.
pub const FILE_NO_HASHER: &str = "no_hasher";
/// The file was readable and its bytes could not be encoded for transport.
pub const FILE_ENCODE_FAILED: &str = "encode_failed";

/// Host digest and local digest agree over the decoded bytes.
pub const INTEGRITY_VERIFIED: &str = "verified";
/// The two digests disagree, or the payload did not decode. Nothing is
/// written.
pub const INTEGRITY_MISMATCH: &str = "mismatch";
/// No bytes were transferred, so there was nothing to verify.
pub const INTEGRITY_UNVERIFIED: &str = "unverified";

/// The largest file this command will move.
///
/// Sized for what it is for: operator scripts, unit files, launch wrappers and
/// configuration — the unversioned text a repository should have been holding
/// all along. A release artifact belongs in the object store, travels with a
/// published digest, and has `stado storage` and `service update` to move it;
/// routing one through a control-plane process's `stdout` would be a second,
/// worse delivery path for bytes that already have one.
pub const MAX_FETCH_BYTES: u64 = 1_048_576;

/// Everything the remote script reported about one fetched file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FetchReport {
    /// The absolute path on the host, as the host resolved it. Empty on a
    /// refusal that happened before any path was accepted.
    pub path: String,
    /// [`FILE_READ`] or one of the refusal words above.
    pub file_state: String,
    /// Why, in the host's words, for any state that is not [`FILE_READ`].
    pub detail: String,
    /// The file's permission bits as the host prints them (`600`, `700`), or
    /// `unknown`.
    pub mode: String,
    /// Whether the mode denies group and other entirely.
    pub owner_only: bool,
    /// The file's size in bytes, as `stat` reported it before the read.
    pub bytes: u64,
    /// The SHA-256 the HOST computed over the file itself, lowercase hex.
    /// Empty when nothing was read.
    pub digest: String,
    /// The file's bytes, base64, on one line. Empty when nothing was read.
    #[serde(default)]
    pub content_b64: String,
}

/// A fetch that arrived, decoded and verified: the bytes, and everything the
/// host said about the file they came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedFile {
    /// The host's own report.
    pub report: FetchReport,
    /// The decoded bytes, byte-exact.
    pub content: Vec<u8>,
    /// The SHA-256 recomputed HERE over [`Self::content`], lowercase hex.
    pub local_digest: String,
    /// [`INTEGRITY_VERIFIED`], [`INTEGRITY_MISMATCH`], or
    /// [`INTEGRITY_UNVERIFIED`].
    pub integrity: &'static str,
}

impl FetchedFile {
    /// True only when the file came back whole and both ends agree on its
    /// bytes.
    pub fn ok(&self) -> bool {
        self.report.file_state == FILE_READ && self.integrity == INTEGRITY_VERIFIED
    }

    /// Why this fetch cannot be believed, or `None` when it can.
    ///
    /// A file that was never opened and a file whose digests disagree are
    /// different failures, and both are failures: this is the one place that
    /// decides so, and every caller reads it rather than re-deriving it.
    pub fn failure(&self, host: &str) -> Option<String> {
        if self.report.file_state != FILE_READ {
            return Some(format!(
                "{host}: {} — {}",
                self.report.file_state,
                if self.report.detail.is_empty() {
                    "no detail"
                } else {
                    &self.report.detail
                }
            ));
        }
        if self.integrity != INTEGRITY_VERIFIED {
            return Some(format!(
                "{host}: the file arrived and its bytes are not what the host hashed \
                 (host {}, local {}); nothing was written",
                if self.report.digest.is_empty() {
                    "-"
                } else {
                    &self.report.digest
                },
                if self.local_digest.is_empty() {
                    "-"
                } else {
                    &self.local_digest
                }
            ));
        }
        None
    }

    /// The fetch as a `--json` report, in [`super::host_inventory`]'s report
    /// shape.
    ///
    /// The bytes are NOT in it. A JSON report is what an operator pastes into
    /// a ticket and what a script pipes into a log; the payload's destination
    /// is the file the caller asked for, and duplicating it here would be a
    /// second uncontrolled copy of exactly the content this command exists to
    /// handle carefully.
    pub fn to_report(&self, target: &ComputeTarget, unit: &str) -> Map<String, Value> {
        let mut object = Map::new();
        object.insert("host".to_string(), json!(target.name));
        object.insert("unit".to_string(), json!(unit));
        object.insert("status".to_string(), json!(OK_STATUS));
        object.insert("path".to_string(), json!(self.report.path));
        object.insert("file_state".to_string(), json!(self.report.file_state));
        object.insert("detail".to_string(), json!(self.report.detail));
        object.insert("mode".to_string(), json!(self.report.mode));
        object.insert("owner_only".to_string(), json!(self.report.owner_only));
        object.insert("bytes".to_string(), json!(self.report.bytes));
        object.insert("fetched_bytes".to_string(), json!(self.content.len()));
        object.insert("host_digest".to_string(), json!(self.report.digest));
        object.insert("local_digest".to_string(), json!(self.local_digest));
        object.insert("integrity".to_string(), json!(self.integrity));
        object
    }
}

/// The remote program.
///
/// One `stat`, one hash, one base64. Nothing here parses or classifies the
/// content: this command's whole contract is that the bytes arrive unaltered,
/// and a script that looked at them would be a place for that to stop being
/// true.
const REMOTE_FETCH_BODY: &str = r##"set -eu
LC_ALL=C
export LC_ALL

home=$HOME
decode=-D
if [ "$(uname)" = "Linux" ]; then decode=--decode; fi
fetch_path=$(printf '%s' '@FETCH_PATH_B64@' | /usr/bin/base64 "$decode")
max_bytes=@MAX_FETCH_BYTES@

# Every field below is either a compile-time constant of this script, a
# digit string, a mode string this script itself validated, or base64 — so the
# payload can never carry host text that breaks it. The path is the one field
# that could, which is why it travels back base64 too and is decoded here.
report() {
  printf '{"path":"%s","file_state":"%s","detail":"%s","mode":"%s","owner_only":%s,"bytes":%s,"digest":"%s","content_b64":"%s"}\n' \
    "$1" "$2" "$3" "$4" "$5" "$6" "$7" "$8"
}

# A refusal is a complete report with an explicit state, not an error exit:
# the caller has to be able to tell "this path is a symlink" from "the channel
# broke".
refuse() {
  report '' "$1" "$2" unknown false 0 '' ''
  exit 0
}

# The $HOME-confinement prelude of `service_env_file.rs`, word for word. The
# command that COPIES a managed file must accept exactly the paths the commands
# that READ and WRITE one accept; a copier with a wider rule would be a
# file-read primitive wearing a service verb's name.
case "$fetch_path" in
  '$HOME'/*) fetch_path="$home/${fetch_path#\$HOME/}" ;;
  "$home"/*) ;;
  /*) refuse refused_outside_home 'the target must be inside the target home' ;;
  *) fetch_path="$home/$fetch_path" ;;
esac
case "$fetch_path" in "$home"/*) ;; *) refuse refused_outside_home 'the target must be inside the target home' ;; esac
# -L before -f, never the other way round: -f follows the link, so a symlink
# would be reported as a present file and its target copied instead.
if [ -L "$fetch_path" ]; then
  refuse refused_symlink 'the target is a symlink and was not followed'
fi
if [ ! -f "$fetch_path" ]; then
  refuse missing 'no regular file at the target'
fi
parent=$(/usr/bin/dirname "$fetch_path")
real_parent=$(/usr/bin/python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$parent")
if ! /usr/bin/python3 -c 'import os,sys; home=os.path.realpath(sys.argv[1]); parent=sys.argv[2]; sys.exit(0 if os.path.commonpath((home,parent)) == home else 1)' "$home" "$real_parent"; then
  refuse refused_outside_home 'the resolved target leaves the target home'
fi
if [ ! -r "$fetch_path" ]; then
  refuse unreadable 'the login user cannot read the target'
fi

path_b64=$(printf '%s' "$fetch_path" | /usr/bin/base64 | /usr/bin/tr -d '\n')

# Both stat(1) dialects, BSD first, exactly as `service_env_file.rs` probes
# them. Neither opens the file.
if facts=$(/usr/bin/stat -f '%z %Lp' "$fetch_path" 2>/dev/null); then
  :
elif facts=$(/usr/bin/stat -c '%s %a' "$fetch_path" 2>/dev/null); then
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

# Refused before the read, not truncated during it. A prefix would hash
# consistently at both ends and the caller would commit half a program.
if [ "$bytes" -gt "$max_bytes" ]; then
  report "$path_b64" refused_too_large "the file is $bytes bytes and the limit is $max_bytes" \
    "$mode" "$owner_only" "$bytes" '' ''
  exit 0
fi

# The digest is the host's own, over the file itself, before any encoding.
# `shasum` where macOS keeps it, `sha256sum` where Linux keeps it, and NEVER a
# fabricated or skipped digest: the whole guarantee of this command is that two
# independently computed hashes of the same bytes agree.
digest=''
if [ -x /usr/bin/shasum ]; then
  digest=$(/usr/bin/shasum -a 256 "$fetch_path" | /usr/bin/awk '{print $1}')
elif command -v sha256sum >/dev/null 2>&1; then
  digest=$(sha256sum "$fetch_path" | /usr/bin/awk '{print $1}')
fi
case "$digest" in
  [0-9a-f]*) ;;
  *) digest='' ;;
esac
if [ -z "$digest" ]; then
  report "$path_b64" no_hasher 'the host has neither shasum nor sha256sum, so no digest could be computed' \
    "$mode" "$owner_only" "$bytes" '' ''
  exit 0
fi

# One line, always: `base64` wraps at 76 columns on some hosts and not others,
# and the report is parsed as a single JSON line.
if ! content=$(/usr/bin/base64 < "$fetch_path" | /usr/bin/tr -d '\n'); then
  report "$path_b64" encode_failed 'the file was readable and its bytes could not be encoded' \
    "$mode" "$owner_only" "$bytes" "$digest" ''
  exit 0
fi

report "$path_b64" read '' "$mode" "$owner_only" "$bytes" "$digest" "$content"
"##;

/// The remote program for one file, with this request's path bound in.
///
/// The path travels base64-encoded inside the script's own body, never in an
/// argument vector, for the same reason `env-set` encodes its value: the
/// script text is the only thing that reaches the host.
pub fn remote_fetch_script(fetch_path: &str) -> String {
    REMOTE_FETCH_BODY
        .replace("@FETCH_PATH_B64@", &STANDARD.encode(fetch_path.as_bytes()))
        .replace("@MAX_FETCH_BYTES@", &MAX_FETCH_BYTES.to_string())
}

/// Parse the script's one line of JSON.
///
/// The LAST line starting with `{` is the payload, for the reason
/// [`super::service_env_file::parse_env_file`] gives: a login shell that greets
/// its callers must not turn a healthy host into a parse error.
pub fn parse_fetch(stdout: &str) -> Result<FetchReport, DeployError> {
    let payload = stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with('{'))
        .ok_or_else(|| DeployError("file fetch script produced no JSON report".to_string()))?;
    let mut report: FetchReport = serde_json::from_str(payload).map_err(|error| {
        DeployError(format!(
            "file fetch script did not return the expected JSON: {error}"
        ))
    })?;
    // The host answers with the path base64-encoded so no filename can break
    // the report; it is only ever a path this command sent, so a payload that
    // does not decode is a broken channel and not a filename question.
    if !report.path.is_empty() {
        let decoded = STANDARD
            .decode(report.path.as_bytes())
            .map_err(|error| {
                DeployError(format!("file fetch returned an unreadable path: {error}"))
            })
            .and_then(|bytes| {
                String::from_utf8(bytes).map_err(|error| {
                    DeployError(format!("file fetch returned a non-UTF-8 path: {error}"))
                })
            })?;
        report.path = decoded;
    }
    Ok(report)
}

/// The lowercase hex SHA-256 of `content`.
pub fn digest_of(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut text, byte| {
            use std::fmt::Write as _;
            let _ = write!(text, "{byte:02x}");
            text
        })
}

/// Decode a report's payload and judge it against the host's own digest.
///
/// Split out from the transport so the one property this command sells —
/// end-to-end integrity — is testable over a report this process did not
/// fetch. A payload that does not decode is [`INTEGRITY_MISMATCH`] and not an
/// error: it is exactly the same finding as a digest that disagrees, and a
/// caller that has to handle it separately will handle it wrongly.
pub fn verify(report: FetchReport) -> FetchedFile {
    if report.file_state != FILE_READ {
        return FetchedFile {
            report,
            content: Vec::new(),
            local_digest: String::new(),
            integrity: INTEGRITY_UNVERIFIED,
        };
    }
    let Ok(content) = STANDARD.decode(report.content_b64.as_bytes()) else {
        return FetchedFile {
            report,
            content: Vec::new(),
            local_digest: String::new(),
            integrity: INTEGRITY_MISMATCH,
        };
    };
    let local_digest = digest_of(&content);
    // The size the host stat'd is compared too. A channel that dropped a whole
    // trailing chunk on a byte boundary would produce a shorter payload whose
    // own digest is self-consistent; only the host's digest catches that, and
    // only the size names it.
    let integrity = if local_digest == report.digest && content.len() as u64 == report.bytes {
        INTEGRITY_VERIFIED
    } else {
        INTEGRITY_MISMATCH
    };
    FetchedFile {
        report,
        content,
        local_digest,
        integrity,
    }
}

/// Fetch one already-resolved registry host's file, whole and verified.
///
/// Split out from the CLI for the reason
/// [`super::service_env_file::read_env_file`] is: the whole fetch is
/// exercisable through the [`Runner`] seam without a registry.
pub async fn fetch_file(
    target: &ComputeTarget,
    fetch_path: &str,
    runner: &Runner,
) -> Result<FetchedFile, DeployError> {
    let script = remote_fetch_script(fetch_path);
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(format!(
            "{}: {}",
            target.name,
            host_channel::last_error_line(&output, "ssh failed")
        )));
    }
    Ok(verify(parse_fetch(&output.stdout)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_report(bytes: &[u8]) -> FetchReport {
        FetchReport {
            path: "/Users/charles/.stado/bin/weles-release-cutover".to_string(),
            file_state: FILE_READ.to_string(),
            detail: String::new(),
            mode: "700".to_string(),
            owner_only: true,
            bytes: bytes.len() as u64,
            digest: digest_of(bytes),
            content_b64: STANDARD.encode(bytes),
        }
    }

    #[test]
    fn a_whole_payload_verifies_and_decodes_byte_exact() {
        // The bytes `env-show` cannot report: a quote, a backslash, a tab and a
        // non-ASCII byte are exactly what its sanitizer replaces with `?`.
        let bytes = b"#!/bin/bash\nsed -E \"/^WC_SKARBIEC_URL=/d\" \\\n\t--\xc3\xa9\n";
        let fetched = verify(read_report(bytes));
        assert_eq!(fetched.integrity, INTEGRITY_VERIFIED);
        assert!(fetched.ok());
        assert_eq!(fetched.content, bytes);
        assert_eq!(fetched.failure("charless-mac-mini"), None);
    }

    #[test]
    fn a_truncated_payload_is_a_mismatch_and_says_both_digests() {
        let bytes = b"#!/bin/bash\nexit 0\n";
        let mut report = read_report(bytes);
        report.content_b64 = STANDARD.encode(&bytes[..4]);
        let fetched = verify(report);
        assert_eq!(fetched.integrity, INTEGRITY_MISMATCH);
        assert!(!fetched.ok());
        let failure = fetched.failure("charless-mac-mini").unwrap();
        assert!(failure.contains(&fetched.local_digest), "{failure}");
        assert!(failure.contains("nothing was written"), "{failure}");
    }

    #[test]
    fn a_payload_matching_its_digest_at_the_wrong_size_is_still_a_mismatch() {
        // The digest is the host's word about the file; `bytes` is `stat`'s.
        // Two host-side answers that disagree mean the file changed under the
        // read, and a fetch that reported `verified` there would hand the
        // caller bytes no single version of the file ever had.
        let bytes = b"one";
        let mut report = read_report(bytes);
        report.bytes = 99;
        assert_eq!(verify(report).integrity, INTEGRITY_MISMATCH);
    }

    #[test]
    fn a_refusal_carries_no_bytes_and_is_never_verified() {
        let report = FetchReport {
            path: String::new(),
            file_state: FILE_REFUSED_SYMLINK.to_string(),
            detail: "the target is a symlink and was not followed".to_string(),
            mode: "unknown".to_string(),
            owner_only: false,
            bytes: 0,
            digest: String::new(),
            content_b64: String::new(),
        };
        let fetched = verify(report);
        assert_eq!(fetched.integrity, INTEGRITY_UNVERIFIED);
        assert!(fetched.content.is_empty());
        let failure = fetched.failure("charless-mac-mini").unwrap();
        assert!(failure.contains(FILE_REFUSED_SYMLINK), "{failure}");
        assert!(failure.contains("was not followed"), "{failure}");
    }

    #[test]
    fn the_path_comes_back_base64_and_is_decoded() {
        let payload = format!(
            r#"{{"path":"{}","file_state":"read","detail":"","mode":"700","owner_only":true,"bytes":3,"digest":"{}","content_b64":"{}"}}"#,
            STANDARD.encode("/Users/charles/a b\"c"),
            digest_of(b"one"),
            STANDARD.encode("one"),
        );
        let report = parse_fetch(&format!("Welcome to macOS\n{payload}\n")).unwrap();
        assert_eq!(report.path, "/Users/charles/a b\"c");
        assert_eq!(verify(report).integrity, INTEGRITY_VERIFIED);
    }

    #[test]
    fn the_script_carries_the_path_only_base64_and_never_literally() {
        let script = remote_fetch_script("$HOME/.stado/bin/weles-release-cutover");
        assert!(!script.contains("weles-release-cutover"), "{script}");
        assert!(script.contains(&STANDARD.encode("$HOME/.stado/bin/weles-release-cutover")));
        assert!(script.contains(&MAX_FETCH_BYTES.to_string()));
    }
}
