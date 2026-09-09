//! The path- and identity-free public form of a cleanup report.

use std::path::Path;

use serde_json::{Map, Value};

use crate::providers::local::disk_cleanup::janitor::pass::lock::file::{lock_contended, open_lock};
use crate::providers::local::disk_cleanup::janitor::pass::lock::{ensure_state_dir, secure_home};
use crate::providers::local::disk_cleanup::janitor::state::error::JanitorError;
use crate::providers::local::disk_cleanup::janitor::state::read_state;
use crate::providers::local::disk_cleanup::janitor::{MAX_ERRORS, STATE_VERSION};
use crate::providers::local::disk_cleanup::{
    backup_twins, chromium_clones, queue_workdirs, release_store,
};

// ---------------------------------------------------------------------------
// sanitized public report (Python `_sanitize_report` and helpers)
// ---------------------------------------------------------------------------

/// Python `_PUBLIC_OUTCOMES`.
const PUBLIC_OUTCOMES: [&str; 13] = [
    "never_run",
    "invalid_or_unavailable_policy",
    "lock_busy",
    "interval_noop",
    "healthy_noop",
    "report_only",
    "lock_recovery_report_only",
    "blocked_running_jobs",
    "reclaimed_target",
    "reclaimed_progress",
    "cap_reached",
    "partial_error",
    "no_eligible_items",
];

/// Public beacon reason codes. Private operator reports retain the complete
/// recorded pass, including reasons absent from this legacy projection.
const PUBLIC_SKIP_REASONS: [&str; 17] = [
    "active_jobs",
    "blob_link_count_uncertain",
    "byte_cap",
    "cache_locked",
    "incomplete_repository",
    "lock_root_absent",
    "not_run_directory",
    "reserved_or_hidden",
    "root_absent",
    "root_changed",
    "same_file_as_primary",
    "scan_cap",
    "scan_deadline",
    "stat_failed",
    "too_young",
    "unsafe_owner_or_device",
    "upload_proof_unavailable_v1",
];

/// Python `_public_nonnegative`: ints only (never bools), floored at 0.
fn public_nonnegative(value: Option<&Value>) -> Option<i64> {
    match value {
        Some(Value::Number(n)) => n.as_i64().map(|v| v.max(0)),
        _ => None,
    }
}

fn public_cleaner(value: Option<&Value>) -> Value {
    let source = value.and_then(Value::as_object);
    let get = |key: &str| source.and_then(|map| map.get(key));
    let mut skipped = Map::new();
    if let Some(skipped_source) = get("skipped").and_then(Value::as_object) {
        for reason in PUBLIC_SKIP_REASONS {
            if let Some(count) = public_nonnegative(skipped_source.get(reason)) {
                if count != 0 {
                    skipped.insert(reason.to_string(), Value::from(count));
                }
            }
        }
    }
    serde_json::json!({
        "scanned_items": public_nonnegative(get("scanned_items")).unwrap_or(0),
        "eligible_items": public_nonnegative(get("eligible_items")).unwrap_or(0),
        "deleted_items": public_nonnegative(get("deleted_items")).unwrap_or(0),
        "expected_bytes": public_nonnegative(get("expected_bytes")).unwrap_or(0),
        "actual_free_delta_bytes": public_nonnegative(get("actual_free_delta_bytes")).unwrap_or(0),
        "skipped": Value::Object(skipped),
    })
}

/// Python `_public_timestamp`: bounded ISO-8601 strings only; returns the
/// re-serialized parse (or None).
fn public_timestamp(value: Option<&Value>) -> Option<String> {
    let text = match value {
        Some(Value::String(s)) if s.len() <= 48 => s,
        _ => return None,
    };
    parse_isoformat(text)
}

/// Python `datetime.fromisoformat(value.replace("Z", "+00:00"))` followed
/// by `.isoformat()`: accept the aware forms we emit plus naive forms,
/// reject everything else.
fn parse_isoformat(text: &str) -> Option<String> {
    let replaced = text.replace('Z', "+00:00");
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(&replaced) {
        let micros = dt.timestamp_subsec_micros();
        if micros == 0 {
            return Some(dt.format("%Y-%m-%dT%H:%M:%S%:z").to_string());
        }
        return Some(dt.format("%Y-%m-%dT%H:%M:%S%.6f%:z").to_string());
    }
    for fmt in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
    ] {
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(text, fmt) {
            let micros = dt.and_utc().timestamp_subsec_micros();
            if micros == 0 {
                return Some(dt.format("%Y-%m-%dT%H:%M:%S").to_string());
            }
            return Some(dt.format("%Y-%m-%dT%H:%M:%S%.6f").to_string());
        }
    }
    None
}

/// Return the stable public report without host, path, or policy identity
/// data. Python `_sanitize_report`.
pub fn sanitize_report(value: &Value, lock_busy: bool) -> Value {
    let source = value.as_object();
    let get = |key: &str| source.and_then(|map| map.get(key));
    let cleaners = get("cleaners").and_then(Value::as_object);
    let caps = get("caps").and_then(Value::as_object);
    let mut safe_errors = Vec::new();
    if let Some(errors) = get("errors").and_then(Value::as_array) {
        for item in errors.iter().take(MAX_ERRORS) {
            if let Some(item) = item.as_str() {
                if !item.is_empty() && item.len() <= 128 && is_safe_error(item) {
                    safe_errors.push(Value::from(item));
                }
            }
        }
    }
    let outcome_raw = get("outcome").and_then(Value::as_str).unwrap_or("");
    let outcome = if lock_busy {
        "lock_busy"
    } else if PUBLIC_OUTCOMES.contains(&outcome_raw) {
        outcome_raw
    } else {
        "never_run"
    };
    let mode = match get("mode").and_then(Value::as_str) {
        Some(m @ ("off" | "report" | "enforce")) => Some(m),
        _ => None,
    };
    let cap = |name: &str| caps.and_then(|c| c.get(name)) == Some(&Value::Bool(true));
    // A pass that did not reach its cleaners carries no table
    // ([`CleanupReport::scanned`]), and the public form has to keep saying so:
    // filling the six sections with zeros here would rebuild, one layer out,
    // exactly the "did not run" that reads as "nothing needed doing".
    let public_cleaners = match cleaners {
        None => Value::Null,
        Some(_) => serde_json::json!({
            "huggingface_cache": public_cleaner(cleaners.and_then(|c| c.get("huggingface_cache"))),
            "weles_recordings": public_cleaner(cleaners.and_then(|c| c.get("weles_recordings"))),
            "build_caches": public_cleaner(cleaners.and_then(|c| c.get("build_caches"))),
            chromium_clones::CLEANER: public_cleaner(
                cleaners.and_then(|c| c.get(chromium_clones::CLEANER)),
            ),
            queue_workdirs::CLEANER: public_cleaner(
                cleaners.and_then(|c| c.get(queue_workdirs::CLEANER)),
            ),
            backup_twins::CLEANER: public_cleaner(
                cleaners.and_then(|c| c.get(backup_twins::CLEANER)),
            ),
            release_store::CLEANER: public_cleaner(
                cleaners.and_then(|c| c.get(release_store::CLEANER)),
            ),
        }),
    };
    // The declared cleaners the pass never reached, kept in the public form
    // because `stado space report` may read it on another
    // machine. Filtered to the six known cleaner names: this crosses a host
    // boundary into an operator's terminal, and every other field here is
    // bounded for the same reason.
    let public_unscanned: Vec<Value> = get("unscanned_cleaners")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .filter(|name| {
                    matches!(
                        *name,
                        "huggingface_cache" | "weles_recordings" | "build_caches"
                    ) || *name == chromium_clones::CLEANER
                        || *name == queue_workdirs::CLEANER
                        || *name == backup_twins::CLEANER
                })
                .map(Value::from)
                .collect()
        })
        .unwrap_or_default();
    serde_json::json!({
        "version": STATE_VERSION,
        "mode": mode,
        "check_interval_seconds": public_nonnegative(get("check_interval_seconds")),
        "started_at": public_timestamp(get("started_at")),
        "duration_ms": public_nonnegative(get("duration_ms")).unwrap_or(0),
        "store_wait_ms": public_nonnegative(get("store_wait_ms")).unwrap_or(0),
        "outcome": outcome,
        "free_bytes_before": public_nonnegative(get("free_bytes_before")),
        "free_bytes_after": public_nonnegative(get("free_bytes_after")),
        "low_bytes": public_nonnegative(get("low_bytes")),
        "target_bytes": public_nonnegative(get("target_bytes")),
        "pressure_active": get("pressure_active").and_then(Value::as_bool),
        "cleaners": public_cleaners,
        "unscanned_cleaners": public_unscanned,
        "caps": {
            "bytes": cap("bytes"),
            "items": cap("items"),
            "scan": cap("scan"),
            "deadline": cap("deadline"),
        },
        "lock_busy": lock_busy || get("lock_busy") == Some(&Value::Bool(true)),
        "active_job_count": public_nonnegative(get("active_job_count")).unwrap_or(0),
        "last_success_at": public_timestamp(get("last_success_at")),
        "errors": safe_errors,
    })
}

/// Python's `area, sep, code = item.partition(":")` + the alnum checks
/// (underscore-stripped; empty remainder is not alnum).
fn is_safe_error(item: &str) -> bool {
    let Some((area, code)) = item.split_once(':') else {
        return false;
    };
    let alnum = |s: &str| {
        let stripped: String = s.chars().filter(|c| *c != '_').collect();
        !stripped.is_empty() && stripped.chars().all(char::is_alphanumeric)
    };
    alnum(area) && alnum(code)
}

/// Return the path- and identity-free public form of a cleanup report.
/// Python `sanitize_cleanup_report`.
pub fn sanitize_cleanup_report(report: &Value) -> Value {
    sanitize_report(report, false)
}

/// Read the owner-controlled state under a shared, no-follow-safe lock.
/// Python `read_cleanup_state`.
pub fn read_cleanup_state_in(home: &Path) -> Result<Value, JanitorError> {
    let home = secure_home(home)?;
    let state_dir = ensure_state_dir(&home)?;
    let lock = open_lock(&state_dir)?;
    match fs2::FileExt::try_lock_shared(&lock) {
        Ok(()) => {}
        Err(exc) if lock_contended(&exc) => {
            return Ok(sanitize_report(&Value::Object(Map::new()), true));
        }
        Err(exc) => return Err(exc.into()),
    }
    let state = read_state(&state_dir)?;
    let report = state.get("report").cloned().unwrap_or(Value::Null);
    Ok(sanitize_report(&report, false))
}

/// Python `read_cleanup_state` at the real home.
pub fn read_cleanup_state() -> Result<Value, JanitorError> {
    read_cleanup_state_in(&crate::config_file::expand_tilde("~"))
}
