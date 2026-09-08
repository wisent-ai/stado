//! What the janitor did, as the screens read it: the recorded disk pass, the
//! memory pass beside it, and the one interval-gated run the GUI can ask for.
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

/// The two janitor passes as one document, which is what the graphical
/// surface reads. `memory_reclaim` is added beside the disk report rather
/// than behind a second route: the screens ask one question — what did this
/// host's janitor do — and two routes would let one of them answer while the
/// other was down.
fn cleanup_document() -> Value {
    let home = crate::config_file::expand_tilde("~");
    let mut report = last_report();
    let memory = crate::providers::local::host_memory::report::last_report_in(&home);
    if let Some(map) = report.as_object_mut() {
        map.insert("memory_reclaim".to_string(), memory);
    }
    report
}

/// `GET /api/cleanup.json`
pub(crate) fn get_cleanup() -> Response {
    send_json(
        http_status("200"),
        &json!({"ok": true, "service": "disk-cleanup", "report": cleanup_document()}),
    )
}

/// `POST /api/cleanup/run`
///
/// One interval-gated pass through the janitor's own entry point, so the mode,
/// the watermarks, the budgets and the lock are the registry's — a run asked
/// for from the GUI is the same pass the timer would have made, not a second
/// implementation of it.
pub(crate) async fn run_cleanup() -> Response {
    let report = crate::providers::local::disk_cleanup::run_cleanup_once(
        0,
        false,
        crate::providers::local::disk_cleanup::CleanupWriter::Cli,
        &mut |_message| {},
    )
    .await;
    send_json(
        http_status("200"),
        &json!({
            "ok": true,
            "service": "disk-cleanup",
            "report": crate::providers::local::disk_cleanup::sanitize_cleanup_report(&report),
        }),
    )
}
