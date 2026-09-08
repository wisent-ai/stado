//! The pipeline: parse the host's one line of JSON, decode it, judge it
//! against the host's own digest, and drive the whole thing over the channel.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use sha2::{Digest, Sha256};

use super::outcomes::{FILE_READ, INTEGRITY_MISMATCH, INTEGRITY_UNVERIFIED, INTEGRITY_VERIFIED};
use super::remote_script::remote_fetch_script;
use super::report::{FetchReport, FetchedFile};
use crate::deploy::{host_channel, DeployError, Runner};
use crate::targets::ComputeTarget;

#[cfg(test)]
mod integrity_end_to_end;

/// Parse the script's one line of JSON.
///
/// The LAST line starting with `{` is the payload, for the reason
/// [`super::service_env_file::parse_env_file`](crate::deploy::service_env_file::parse_env_file)
/// gives: a login shell that greets
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
/// [`super::service_env_file::read_env_file`](crate::deploy::service_env_file::read_env_file)
/// is: the whole fetch is
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
