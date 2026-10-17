//! The Boat API constants and the sandbox-id pattern.
//!
//! The public API base, the not-found status, the statuses a caller may
//! retry and the sandbox-id pattern, read by the `http` and `client`
//! siblings and by the `payload::parse` component in this tree.

use std::sync::LazyLock;

use regex::Regex;
use reqwest::StatusCode;

/// Boat Public API v1, as its OpenAPI document names its server.
pub const DEFAULT_BOX_API_URL: &str = "https://boat.dev/api/v1";
/// The status Boat answers for a sandbox it does not hold.
pub const HTTP_NOT_FOUND: u16 = StatusCode::NOT_FOUND.as_u16();

/// Whether Boat's answer `status` is one a caller may send again unchanged:
/// a rate limit, or a server or gateway that failed to complete the request.
/// A server that does not implement the request fails the same way again.
pub fn transient_status(status: u16) -> bool {
    StatusCode::from_u16(status).is_ok_and(|status| {
        status == StatusCode::TOO_MANY_REQUESTS
            || (status.is_server_error() && status != StatusCode::NOT_IMPLEMENTED)
    })
}

/// A Boat sandbox id: `bx_` and eight letters of Boat's alphabet, anchored.
pub fn box_id_pattern() -> &'static Regex {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^bx_[23456789abcdefghjkmnpqrstuvwxyz]{8}$").expect("static regex compiles")
    });
    &RE
}
