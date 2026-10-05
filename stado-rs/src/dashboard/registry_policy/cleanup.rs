//! What the janitor did, as the screens read it: the recorded pass and the
//! one pass the GUI can ask for.
//!
//! Split out of `write.rs` so neither file passes three hundred lines. The
//! route surface is unchanged; `mod.rs` re-exports both handlers.

use super::*;

/// The janitor's last recorded pass, sanitized.
fn last_report() -> Value {
    let home = crate::config_file::expand_tilde("~");
    let path = home.join(crate::providers::local::disk_cleanup::state_relative_path());
    let recorded = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|state| state.get("report").cloned())
        .unwrap_or(Value::Null);
    crate::providers::local::disk_cleanup::sanitize_cleanup_report(&recorded)
}

/// `GET /api/cleanup.json`
pub(crate) fn get_cleanup() -> Response {
    send_json(
        http_status(reqwest::StatusCode::OK),
        &json!({"ok": true, "service": "disk-cleanup", "report": last_report()}),
    )
}

/// `POST /api/cleanup/run`
///
/// One pass through the janitor's own entry point, so the rule and the lock
/// are the same as the timer's — a run asked for from the GUI is the same
/// pass the timer would have made, not a second implementation of it.
pub(crate) async fn run_cleanup() -> Response {
    let report = crate::providers::local::disk_cleanup::run_cleanup_once(
        0,
        crate::providers::local::disk_cleanup::CleanupWriter::Cli { every: None },
        &mut |_message| {},
    )
    .await;
    send_json(
        http_status(reqwest::StatusCode::OK),
        &json!({
            "ok": true,
            "service": "disk-cleanup",
            "report": crate::providers::local::disk_cleanup::sanitize_cleanup_report(&report),
        }),
    )
}
