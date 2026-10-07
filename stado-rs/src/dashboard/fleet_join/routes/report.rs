//! The shape of the machine's self-report, and what a reported hostname is
//! allowed to be before it becomes an object key.

use serde_json::Value;
use std::num::NonZeroUsize;

/// The machine's self-report. Extra keys are tolerated so the bootstrap
/// script can report more without a lockstep release; `ssh_listening` is the
/// observation it makes today, because a laptop whose owner has not yet
/// granted Remote Login must still be able to file its request.
pub(super) struct Report {
    pub(super) hostname: String,
    pub(super) os: String,
    pub(super) arch: String,
    pub(super) destination: String,
    pub(super) fingerprint: String,
    pub(super) ssh_listening: Option<bool>,
}

pub(super) fn parse_report(body: &[u8], field_bytes: NonZeroUsize) -> Result<Report, String> {
    let document: Value = serde_json::from_slice(body)
        .map_err(|error| format!("join report body is not JSON: {error}"))?;
    let field = |name: &str, allow_empty: bool| -> Result<String, String> {
        let value = document
            .get(name)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("join report field {name} must be a string"))?
            .trim();
        if value.is_empty() && !allow_empty {
            return Err(format!("join report field {name} must not be empty"));
        }
        if value.len() > field_bytes.get() {
            return Err(format!(
                "join report field {name} has {} bytes; dashboard.request_limits.fleet_join.field_bytes permits {field_bytes}",
                value.len()
            ));
        }
        Ok(value.to_owned())
    };
    Ok(Report {
        hostname: field("hostname", false)?,
        os: field("os", false)?,
        arch: field("arch", false)?,
        destination: field("destination", false)?,
        // A machine without a fingerprinting tool reports that absence.
        fingerprint: field("installed_key_fingerprint", true)?,
        ssh_listening: document.get("ssh_listening").and_then(Value::as_bool),
    })
}

/// A reported hostname becomes an object key, so it is held to what a machine
/// name can be: no separators, no traversal, nothing that could address a
/// different part of the store.
pub(super) fn valid_hostname(hostname: &str, field_bytes: NonZeroUsize) -> bool {
    !hostname.is_empty()
        && hostname.len() <= field_bytes.get()
        && hostname
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.')
        && !hostname.starts_with('.')
        && !hostname.contains("..")
}
