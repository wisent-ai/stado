//! The two per-run gates the scan consults before it may evict a run.

use std::os::unix::fs::MetadataExt;
use std::path::Path;

use serde_json::Value;

/// True only when the run carries a valid whole-run upload proof and
/// nothing was written into the run afterwards.
///
/// The proof (recordings/<run>/.uploaded.json, written by the Weles worker
/// after a zero-failure mirror) is invalidated by any newer direct child —
/// a file added after the upload means storage is no longer complete.
/// Python `_weles_upload_proof_ok`.
pub(super) fn upload_proof_ok(run_dir: &Path) -> bool {
    let proof_path = run_dir.join(".uploaded.json");
    let text = match std::fs::read_to_string(&proof_path) {
        Ok(text) => text,
        Err(_) => return false,
    };
    let proof: Value = match serde_json::from_str(&text) {
        Ok(proof) => proof,
        Err(_) => return false,
    };
    if !proof.is_object() || proof.get("version") != Some(&Value::from(1)) {
        return false;
    }
    match proof.get("file_count") {
        Some(Value::Number(n)) if n.as_i64().is_some_and(|c| c > 0) => {}
        _ => return false,
    }
    let Some(uploaded_at_raw) = proof.get("uploaded_at").and_then(Value::as_str) else {
        return false;
    };
    let uploaded_at = match parse_iso_timestamp(uploaded_at_raw) {
        Some(ts) => ts,
        None => return false,
    };
    let entries = match std::fs::read_dir(run_dir) {
        Ok(entries) => entries,
        Err(_) => return false,
    };
    for entry in entries {
        let Ok(entry) = entry else { return false };
        if entry.file_name() == ".uploaded.json" {
            continue;
        }
        // DirEntry::metadata does not follow symlinks
        // (entry.stat(follow_symlinks=False) parity).
        match entry.metadata() {
            Ok(info) => {
                if info.mtime() as f64 > uploaded_at {
                    return false;
                }
            }
            Err(_) => return false,
        }
    }
    true
}

/// Python `datetime.fromisoformat(raw.replace("Z", "+00:00")).timestamp()`.
fn parse_iso_timestamp(raw: &str) -> Option<f64> {
    let replaced = raw.replace('Z', "+00:00");
    let dt = chrono::DateTime::parse_from_rfc3339(&replaced).ok()?;
    let seconds = dt.timestamp() as f64 + f64::from(dt.timestamp_subsec_nanos()) / 1e9;
    Some(seconds)
}

/// Any direct child fresher than cutoff means the run is likely live
/// (dir mtime alone misses in-place file writes). Errors read as
/// inactive: the outer age gate already passed.
/// Python `_weles_run_active`.
pub(super) fn run_active(path: &Path, cutoff: f64) -> bool {
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(_) => return false,
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        match entry.metadata() {
            Ok(info) => {
                if info.mtime() as f64 > cutoff {
                    return true;
                }
            }
            Err(_) => continue,
        }
    }
    false
}
