//! The one refusal every failed authorization produces, and the separate
//! answer infrastructure failure gets.

use serde_json::json;

use crate::dashboard::{http_status, send_json, Response};

use super::redeem::Denial;

/// The one refusal every failed authorization produces, kept identical to the
/// CLI's.
const REFUSAL: &str = "invite token is not usable";

// ---------------------------------------------------------------------------
// Responses
// ---------------------------------------------------------------------------

/// The single refusal.
pub(super) fn refuse() -> Response {
    send_json(
        http_status(reqwest::StatusCode::UNAUTHORIZED),
        &json!({"error": REFUSAL}),
    )
}

pub(super) fn unavailable(message: &str) -> Response {
    send_json(
        http_status(reqwest::StatusCode::SERVICE_UNAVAILABLE),
        &json!({"error": message}),
    )
}

pub(super) fn denied(denial: Denial) -> Response {
    match denial {
        Denial::Refused => refuse(),
        Denial::Unavailable(message) => unavailable(message),
    }
}
