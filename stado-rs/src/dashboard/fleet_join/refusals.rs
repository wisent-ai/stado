//! The one refusal every failed authorization produces, the floor it is held
//! to, and the separate answer infrastructure failure gets.

use std::time::{Duration, Instant};

use serde_json::json;

use crate::dashboard::{http_status, send_json, Response};

use super::redeem::Denial;

/// Every refusal takes at least this long, measured from the start of the
/// request, so a rejected code cannot be classified by how fast it failed —
/// including the difference between "rate limited" (no I/O) and "no such
/// invite" (one store read).
const REFUSAL_FLOOR: Duration = Duration::from_millis(250);

/// The one refusal every failed authorization produces, kept identical to the
/// CLI's.
const REFUSAL: &str = "invite token is not usable";

// ---------------------------------------------------------------------------
// Responses
// ---------------------------------------------------------------------------

/// The single refusal, held to a fixed floor on elapsed time.
pub(super) async fn refuse(started: Instant) -> Response {
    let elapsed = started.elapsed();
    if elapsed < REFUSAL_FLOOR {
        tokio::time::sleep(REFUSAL_FLOOR - elapsed).await;
    }
    send_json(http_status("401"), &json!({"error": REFUSAL}))
}

pub(super) fn unavailable(message: &str) -> Response {
    send_json(http_status("503"), &json!({"error": message}))
}

pub(super) async fn denied(started: Instant, denial: Denial) -> Response {
    match denial {
        Denial::Refused => refuse(started).await,
        Denial::Unavailable(message) => unavailable(message),
    }
}
