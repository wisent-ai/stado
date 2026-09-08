//! The queue switch: whether dispatch is running at all.

use crate::doctor::plane::storage::round_trip::STORAGE_REMEDY;
use crate::doctor::{Check, Status};
use crate::queue::{control, JobStorage};

// ---------------------------------------------------------------------------
// 9. Queue control
// ---------------------------------------------------------------------------

pub(in crate::doctor) const CONTROL_ID: &str = "queue-control";
pub(in crate::doctor) const CONTROL_TITLE: &str = "Queue control";
pub(in crate::doctor) const CONTROL_REMEDY: &str =
    "`stado queue pause` / `stado queue resume` own this state";

/// Whether dispatch is paused. A paused queue perfectly explains an idle
/// fleet in front of a full queue, and is invisible everywhere else.
pub(in crate::doctor) async fn check_queue_control(
    store: Option<&JobStorage>,
    store_error: &str,
) -> Check {
    let Some(store) = store else {
        return Check::fail(
            CONTROL_ID,
            CONTROL_TITLE,
            format!(
                "the pause flag lives at {} in the queue store, which could not be \
                 constructed: {store_error}",
                control::CONTROL_BLOB
            ),
            STORAGE_REMEDY,
        );
    };
    match control::read(store).await {
        Err(err) => Check::fail(
            CONTROL_ID,
            CONTROL_TITLE,
            format!("could not read {}: {err}", control::CONTROL_BLOB),
            STORAGE_REMEDY,
        ),
        Ok(state) if state.paused => Check::new(
            CONTROL_ID,
            CONTROL_TITLE,
            Status::Warn,
            // The same one-liner the scheduler and the agent print when
            // they refuse work, so all three say the pause the same way.
            format!("dispatch is PAUSED — {}", state.pause_summary()),
            "`stado queue resume` restarts dispatch",
        ),
        Ok(_) => Check::pass(
            CONTROL_ID,
            CONTROL_TITLE,
            "dispatch is running (not paused)".to_string(),
            CONTROL_REMEDY,
        ),
    }
}
