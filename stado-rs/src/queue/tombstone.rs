//! Live re-submission tracking.
//!
//! Port of `stado/queue/tracking/tombstone.py`. `JobStorage::move_job`
//! calls [`on_transition`] after every state move. When the moved job
//! carries `re_submission_of` and the destination is a terminal state
//! (completed/uploaded/failed), this writes
//! `{fixed,failed_again}/<orig_jid>.json` keyed on the ORIGINAL failed job
//! id. The tracker then answers "is original X fixed?" via a single
//! list diff:
//!
//! ```text
//! still_broken = list(failed/) - list(fixed/)
//! ```
//!
//! instead of a full per-blob rescan.
//!
//! Never raises into the agent loop — a marker write failure is logged but
//! must not crash the state transition.

use crate::models::Job;

use super::json_str;
use super::storage::JobStorage;

/// Marker prefix per terminal destination (Python `_TERMINAL_TO_MARKER`).
fn marker_prefix(to_prefix: &str) -> Option<&'static str> {
    match to_prefix {
        "completed" | "uploaded" => Some("fixed"),
        "failed" => Some("failed_again"),
        _ => None,
    }
}

/// Python `a or b or ""` over optional strings.
fn first_non_empty<'a>(a: Option<&'a str>, b: Option<&'a str>) -> &'a str {
    match (a, b) {
        (Some(a), _) if !a.is_empty() => a,
        (_, Some(b)) if !b.is_empty() => b,
        _ => "",
    }
}

/// Write a tombstone if the move terminates a re-submitted job.
pub async fn on_transition(store: &JobStorage, job: &Job, to_prefix: &str) {
    let orig = job.re_submission_of.as_str();
    if orig.is_empty() {
        return;
    }
    let Some(marker_prefix) = marker_prefix(to_prefix) else {
        return;
    };
    let ts = first_non_empty(job.completed_at.as_deref(), job.failed_at.as_deref());
    // Python `json.dumps({...})` with default separators and dict order.
    let body = format!(
        "{{\"orig_jid\": {}, \"new_jid\": {}, \"new_state\": {}, \"batch_id\": {}, \"ts\": {}}}",
        json_str(orig),
        json_str(&job.job_id),
        json_str(to_prefix),
        json_str(&job.batch_id),
        json_str(ts),
    );
    if let Err(exc) = store
        .upload_text(&format!("{marker_prefix}/{orig}.json"), &body)
        .await
    {
        // Never raise into the agent loop.
        eprintln!("[tombstone] write failed for {orig}: {exc:?}");
    }
}
