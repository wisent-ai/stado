//! The Box API constants and the box-id pattern.
//!
//! Python `DEFAULT_BOX_API_URL`, `DEFAULT_TIMEOUT_SECONDS`,
//! `MAX_JSON_BYTES`, `HTTP_NOT_FOUND`, `TRANSIENT_HTTP` and
//! `BOX_ID_PATTERN`, read by the `http` and `client` siblings and by the
//! `payload::parse` component in this tree.

use std::sync::LazyLock;

use regex::Regex;

/// Python `DEFAULT_BOX_API_URL`.
pub const DEFAULT_BOX_API_URL: &str = "https://ascii.dev/api/box/v1";
/// Python `DEFAULT_TIMEOUT_SECONDS`.
pub const DEFAULT_TIMEOUT_SECONDS: f64 = 70.0;
/// Python `MAX_JSON_BYTES`.
pub const MAX_JSON_BYTES: usize = 65536;
/// Python `HTTP_NOT_FOUND`.
pub const HTTP_NOT_FOUND: u16 = 404;
/// Python `TRANSIENT_HTTP`: statuses the caller may retry.
pub const TRANSIENT_HTTP: [u16; 5] = [429, 500, 502, 503, 504];

/// Python `BOX_ID_PATTERN` (`fullmatch` semantics — the pattern is anchored).
pub fn box_id_pattern() -> &'static Regex {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^bx_[23456789abcdefghjkmnpqrstuvwxyz]{8}$").expect("static regex compiles")
    });
    &RE
}
