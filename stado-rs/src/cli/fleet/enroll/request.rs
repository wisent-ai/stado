//! The enrollment request document: where it lives in the store, the
//! lifecycle markers it carries, the pure builders that produce one and the
//! pure readers that interrogate one.

use crate::targets::normalize_hostname;
use serde_json::{json, Value};

/// Store prefix every enrollment request lives under.
pub(super) const REQUESTS_PREFIX: &str = "enrollments/";
/// Request lifecycle markers.
pub(super) const STATUS_PENDING: &str = "pending";
pub(super) const STATUS_APPROVED: &str = "approved";

/// Map one directly observed operating-system and architecture pair to the
/// closed immutable-release platform table. Both Rust's local constants and
/// the exact `uname` spellings used by remote enrollment are accepted.
pub fn release_platform(os: &str, arch: &str) -> Result<&'static str, String> {
    match (os.trim(), arch.trim()) {
        ("macos", "aarch64") | ("Darwin", "arm64") => Ok("darwin-arm64"),
        ("linux", "x86_64") | ("Linux", "x86_64") => Ok("linux-amd64"),
        (os, arch) => Err(format!(
            "unsupported release platform observation: os={os:?}, arch={arch:?}"
        )),
    }
}

pub(super) fn request_path(hostname: &str) -> String {
    format!("{REQUESTS_PREFIX}{hostname}.json")
}

/// Build the join-request document for this machine. Pure.
pub fn build_request(hostname: &str, os: &str, arch: &str) -> Value {
    json!({
        "hostname": hostname,
        "os": os,
        "arch": arch,
        "kind": "local",
        "requested_at": chrono::Utc::now().to_rfc3339(),
        "status": STATUS_PENDING,
    })
}

/// Build the request an invited machine files: the same document plus the four
/// facts an invite adds — the name the invite reserved, which channel the fleet
/// should come back on, which invite paid for it, and which key the machine
/// says it installed.
///
/// `target_name` is not cosmetic. The invite minted the channel key as
/// `stado-ssh-<target_name>` and told the machine's owner that name, so
/// approving under the machine's own hostname instead would reach for a key
/// nobody ever minted — the channel would fail after the owner had already
/// installed the right key. `destination` is what makes approval a verified
/// enrollment instead of a declaration, so it is the field `approve` branches
/// on. Pure.
pub fn build_invited_request(
    hostname: &str,
    os: &str,
    arch: &str,
    target_name: &str,
    destination: &str,
    invite_id: &str,
    installed_key_fingerprint: &str,
) -> Value {
    let mut request = build_request(hostname, os, arch);
    request["target_name"] = Value::String(target_name.to_string());
    request["destination"] = Value::String(destination.to_string());
    request["invite_id"] = Value::String(invite_id.to_string());
    request["installed_key_fingerprint"] = Value::String(installed_key_fingerprint.to_string());
    request
}

/// The SSH destination an invited machine asked the fleet to come back on, or
/// `None` for today's machine-initiated `join` (which has no channel). Pure.
pub fn request_destination(document: &Value) -> Option<&str> {
    document
        .get("destination")
        .and_then(Value::as_str)
        .filter(|destination| !destination.trim().is_empty())
}

/// The registry name an invite reserved for this machine, if the request came
/// from one. Pure.
pub fn request_target_name(document: &Value) -> Option<&str> {
    document
        .get("target_name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
}

/// The invite that paid for a request, if any. Pure.
pub fn request_invite_id(document: &Value) -> Option<&str> {
    document
        .get("invite_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
}

/// Derive the target name for an approved request: the normalized machine
/// hostname. Pure.
pub fn target_name_for(hostname: &str) -> String {
    normalize_hostname(hostname)
}

/// Parse a stored request and confirm it is still awaiting a decision.
/// Pure.
pub fn pending_request(document: &Value) -> Result<&str, String> {
    let status = document
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if status != STATUS_PENDING {
        return Err(format!(
            "request status is '{status}', not '{STATUS_PENDING}'"
        ));
    }
    document
        .get("hostname")
        .and_then(Value::as_str)
        .filter(|hostname| !hostname.is_empty())
        .ok_or_else(|| "request has no hostname".to_string())
}
