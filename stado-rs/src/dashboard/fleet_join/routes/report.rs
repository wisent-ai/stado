//! The shape of the machine's self-report, and what a reported hostname is
//! allowed to be before it becomes an object key.

use serde_json::Value;

use super::super::MAX_FIELD_BYTES;

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

pub(super) fn parse_report(body: &[u8]) -> Result<Report, &'static str> {
    let document: Value = serde_json::from_slice(body).map_err(|_| "request body is not JSON")?;
    let required = |name: &str| -> Result<String, &'static str> {
        let value = document
            .get(name)
            .and_then(Value::as_str)
            .ok_or("join report is missing a required field")?
            .trim()
            .to_string();
        if value.is_empty() || value.len() > MAX_FIELD_BYTES {
            return Err("join report field is empty or too long");
        }
        Ok(value)
    };
    // A machine with neither ssh-keygen nor openssl cannot fingerprint the
    // key it just installed. That is reported empty, never fabricated.
    let fingerprint = document
        .get("installed_key_fingerprint")
        .and_then(Value::as_str)
        .ok_or("join report is missing a required field")?
        .trim()
        .to_string();
    if fingerprint.len() > MAX_FIELD_BYTES {
        return Err("join report field is empty or too long");
    }
    Ok(Report {
        hostname: required("hostname")?,
        os: required("os")?,
        arch: required("arch")?,
        destination: required("destination")?,
        fingerprint,
        ssh_listening: document.get("ssh_listening").and_then(Value::as_bool),
    })
}

/// A reported hostname becomes an object key, so it is held to what a machine
/// name can be: no separators, no traversal, nothing that could address a
/// different part of the store.
pub(super) fn valid_hostname(hostname: &str) -> bool {
    !hostname.is_empty()
        && hostname.len() <= MAX_FIELD_BYTES
        && hostname
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.')
        && !hostname.starts_with('.')
        && !hostname.contains("..")
}
