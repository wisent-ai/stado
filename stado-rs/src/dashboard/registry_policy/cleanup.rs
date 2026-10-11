//! What the janitor did, as the screens read it: the recorded pass and the
//! one pass the GUI can ask for.
//!
//! Split out of `write.rs` so neither file passes three hundred lines. The
//! route surface is unchanged; `mod.rs` re-exports both handlers.

use super::*;

/// The janitor's last recorded pass, sanitized. A state file that is not
/// there is no pass yet; one that cannot be read, is not JSON or carries no
/// report is refused naming it: read as no pass, the screens showed an
/// empty report.
fn last_report() -> Result<Value, String> {
    let home = crate::config_file::expand_tilde("~");
    let path = home.join(crate::providers::local::disk_cleanup::state_relative_path());
    let recorded = match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str::<Value>(&text)
            .map_err(|error| format!("{} is not JSON: {error}", path.display()))?
            .get("report")
            .cloned()
            .ok_or_else(|| format!("{} carries no report", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Value::Null,
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    Ok(crate::providers::local::disk_cleanup::sanitize_cleanup_report(&recorded))
}

/// `GET /api/cleanup.json`
pub(crate) fn get_cleanup() -> Response {
    match last_report() {
        Ok(report) => send_json(
            http_status(reqwest::StatusCode::OK),
            &json!({"ok": true, "service": "disk-cleanup", "report": report}),
        ),
        Err(error) => send_json(
            http_status(reqwest::StatusCode::INTERNAL_SERVER_ERROR),
            &json!({"ok": false, "service": "disk-cleanup", "error": error}),
        ),
    }
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
