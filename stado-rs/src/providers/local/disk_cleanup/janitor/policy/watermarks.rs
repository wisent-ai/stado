//! The low watermark as the agent asks for it, outside a pass.

use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use serde_json::Value;

use crate::providers::local::disk_cleanup::janitor::pass::lock::euid;
use crate::providers::local::disk_cleanup::janitor::{STATE_NAME, STATE_VERSION};

// ---------------------------------------------------------------------------
// agent-side low-watermark plumbing (Python local_agent helpers)
// ---------------------------------------------------------------------------

/// Python `_validated_report_low_bytes`: a threshold only from a
/// successfully resolved policy report.
pub fn validated_report_low_bytes(report: &Value) -> Option<i64> {
    let digest = report.get("policy_digest").and_then(Value::as_str)?;
    // Python `int(digest, 16)`: arbitrary precision, so any 64-char hex
    // string validates (including high-bit-set digests that overflow u128).
    if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    match report.get("low_bytes") {
        Some(Value::Number(n)) => n.as_i64().filter(|v| *v > 0),
        _ => None,
    }
}

/// Read the last canonical low watermark from janitor-owned safe state
/// (Python `_persisted_disk_low_bytes` at an explicit home; test seam).
///
/// Reuse a threshold only when it came from the janitor's
/// owner-controlled, no-follow state file and the report identifies a
/// validated canonical policy.
pub fn persisted_disk_low_bytes_in(home: &Path) -> Option<i64> {
    let state_path = home.join(".cache").join("wisent-compute").join(STATE_NAME);
    for directory in [
        home.to_path_buf(),
        home.join(".cache"),
        home.join(".cache/wisent-compute"),
    ] {
        let info = std::fs::symlink_metadata(&directory).ok()?;
        if info.file_type().is_symlink() || !info.is_dir() || info.uid() != euid() {
            return None;
        }
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(&state_path)
        .ok()?;
    let info = file.metadata().ok()?;
    if !info.is_file() || info.uid() != euid() || info.len() > 1024 * 1024 {
        return None;
    }
    let mut text = String::new();
    (&file).read_to_string(&mut text).ok()?;
    let state: Value = serde_json::from_str(&text).ok()?;
    if state.get("version")?.as_i64()? != STATE_VERSION {
        return None;
    }
    validated_report_low_bytes(state.get("report")?)
}

/// Python `_persisted_disk_low_bytes` at the real home.
pub fn persisted_disk_low_bytes() -> Option<i64> {
    persisted_disk_low_bytes_in(&crate::config_file::expand_tilde("~"))
}

/// Fail admission closed until both policy threshold and free space are
/// known. Python `_disk_pressure_unresolved`.
pub fn disk_pressure_unresolved(low_bytes: Option<i64>, free_bytes: Option<i64>) -> bool {
    match (low_bytes, free_bytes) {
        (Some(low), Some(free)) => free < low,
        _ => true,
    }
}

/// Whether free space is below the janitor's low watermark, with both numbers
/// known. The janitor's own `report.pressure_active`, asked from outside a pass.
///
/// Separate from [`disk_pressure_unresolved`] because the two answer different
/// questions and one answer for both stopped a host for seven days. Not knowing
/// the threshold is a reason to refuse admission; being under it is a reason to
/// reclaim, and on a host with nothing eligible to delete it is a state no pass
/// can leave, so it must never be the thing that silences a capacity broadcast.
pub fn disk_pressure_active(low_bytes: Option<i64>, free_bytes: Option<i64>) -> bool {
    matches!((low_bytes, free_bytes), (Some(low), Some(free)) if free < low)
}
