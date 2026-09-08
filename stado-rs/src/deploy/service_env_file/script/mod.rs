//! The secret-field delivery: what one read asks for, the remote program that
//! answers it, and the parse of the one line that comes back.
//!
//! The classification that decides what may be shown runs on the host, inside
//! the script this module renders, so a value this command will not show is
//! never put on the wire. Nothing here interprets a value; that is
//! [`diagnostics`](super::diagnostics)'s job, on what the host chose to send.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;

use super::*;
use crate::deploy::DeployError;

mod file_half;
mod listeners_half;
#[cfg(test)]
mod shell_parses;

/// The remote program.
///
/// One `awk` pass does the parsing, the classification and the JSON, and it is
/// the only fork in the file half of this script. The shell-builtin sanitizer
/// [`super::host_inventory`](crate::deploy::host_inventory) uses is right for a hundred short names and wrong
/// here: an env file is hundreds of long values, and a per-character shell loop
/// over all of them is quadratic work for no gain. An `awk` that dies is
/// reported as [`ENTRIES_PARSE_FAILED`] rather than as a file with nothing in
/// it, which is the failure mode that matters.
///
/// The classification lives in `awk`, on the host, because that is what makes
/// "a redacted value never crosses the channel" true rather than merely
/// intended.
///
/// The text is held in two consts, `REMOTE_ENV_FILE_HEAD` and
/// `REMOTE_ENV_FILE_TAIL`, and joined here in that order. The seam is the
/// blank line that already stood between the file section and the socket-table
/// section, so what a host receives is the same script it always was.
fn remote_env_file_body() -> String {
    format!(
        "{}{}",
        file_half::REMOTE_ENV_FILE_HEAD,
        listeners_half::REMOTE_ENV_FILE_TAIL
    )
}

/// What one read of a managed env file asks for.
///
/// `expect` is what lets a WRITER see its own write. `env-set` has just put a
/// value in this file; passing the same key and value here makes the host
/// compare them against that key's effective assignment and answer with one
/// word. Exact for a secret as well as an endpoint, and nothing comes back but
/// the word — which is why this is a parameter of the reader rather than a
/// length comparison in the caller.
pub struct EnvFileRequest<'a> {
    pub env_path: &'a str,
    /// Show this one key's value in full whatever its name suggests.
    pub reveal: Option<&'a str>,
    /// One key and the exact text its effective assignment should hold.
    pub expect: Option<(&'a str, &'a str)>,
}

impl<'a> EnvFileRequest<'a> {
    /// A plain read: no reveal, no expectation.
    pub fn read(env_path: &'a str) -> Self {
        Self {
            env_path,
            reveal: None,
            expect: None,
        }
    }
}

/// The remote program for one env file, with this request's selections bound in.
///
/// Every operand travels base64-encoded inside the request body, never in an
/// argument vector, for the same reason `env-set` encodes its value: the
/// script text is the only thing that reaches the host.
pub fn remote_env_file_script(request: &EnvFileRequest<'_>) -> String {
    let (expect_key, expect_value) = request.expect.unwrap_or_default();
    remote_env_file_body()
        .replace(
            "@ENV_PATH_B64@",
            &STANDARD.encode(request.env_path.as_bytes()),
        )
        .replace(
            "@REVEAL_B64@",
            &STANDARD.encode(request.reveal.unwrap_or_default().as_bytes()),
        )
        .replace("@EXPECT_KEY_B64@", &STANDARD.encode(expect_key.as_bytes()))
        .replace(
            "@EXPECT_VALUE_B64@",
            &STANDARD.encode(expect_value.as_bytes()),
        )
        .replace("@MAX_ENTRIES@", &MAX_ENTRIES.to_string())
        .replace("@MAX_VALUE_CHARS@", &MAX_VALUE_CHARS.to_string())
}

/// Parse the script's one line of JSON.
///
/// The LAST line starting with `{` is the payload, for the reason
/// [`super::host_inventory::parse_inventory`](crate::deploy::host_inventory::parse_inventory) gives: a login shell that greets
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
