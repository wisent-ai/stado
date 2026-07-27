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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::local_file::LocalBackend;
    use std::sync::Arc;

    fn store() -> (tempfile::TempDir, JobStorage) {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(dir.path().to_str().unwrap()).unwrap();
        (dir, JobStorage::with_backend(Arc::new(backend), "local"))
    }

    fn resubmitted_job() -> Job {
        let mut job = Job::new("new1", "echo retry");
        job.created_at = "2026-01-01T00:00:00+00:00".into();
        job.re_submission_of = "orig9".into();
        job.batch_id = "b1".into();
        job.completed_at = Some("2026-01-03T10:00:00+00:00".into());
        job
    }

    #[tokio::test]
    async fn terminal_move_writes_fixed_tombstone() {
        let (_dir, store) = store();
        let job = resubmitted_job();
        store.write_job("queue", &job).await.unwrap();
        store.move_job(&job, "queue", "completed").await.unwrap();
        assert_eq!(
            store
                .download_text("fixed/orig9.json")
                .await
                .unwrap()
                .as_deref(),
            Some(
                "{\"orig_jid\": \"orig9\", \"new_jid\": \"new1\", \"new_state\": \"completed\", \
                 \"batch_id\": \"b1\", \"ts\": \"2026-01-03T10:00:00+00:00\"}"
            )
        );
    }

    #[tokio::test]
    async fn failed_move_writes_failed_again_tombstone() {
        let (_dir, store) = store();
        let mut job = resubmitted_job();
        job.completed_at = None;
        job.failed_at = Some("2026-01-03T11:00:00+00:00".into());
        store.write_job("queue", &job).await.unwrap();
        store.move_job(&job, "queue", "failed").await.unwrap();
        assert_eq!(
            store
                .download_text("failed_again/orig9.json")
                .await
                .unwrap()
                .as_deref(),
            Some(
                "{\"orig_jid\": \"orig9\", \"new_jid\": \"new1\", \"new_state\": \"failed\", \
                 \"batch_id\": \"b1\", \"ts\": \"2026-01-03T11:00:00+00:00\"}"
            )
        );
    }

    #[tokio::test]
    async fn non_terminal_move_and_plain_job_write_nothing() {
        let (_dir, store) = store();
        let job = resubmitted_job();
        store.write_job("queue", &job).await.unwrap();
        store.move_job(&job, "queue", "running").await.unwrap();
        assert!(store.list_paths("fixed/", 0).await.unwrap().is_empty());
        assert!(store
            .list_paths("failed_again/", 0)
            .await
            .unwrap()
            .is_empty());

        // A job without re_submission_of never tombstones.
        let plain = Job::new("plain1", "echo");
        store.write_job("queue", &plain).await.unwrap();
        store.move_job(&plain, "queue", "completed").await.unwrap();
        assert!(store.list_paths("fixed/", 0).await.unwrap().is_empty());
    }
}
