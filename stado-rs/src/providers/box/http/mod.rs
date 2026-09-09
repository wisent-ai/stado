//! Bounded HTTP transport for Box Public API v1.
//!
//! Port of `stado/providers/box/_http.py`. The Python class builds on
//! urllib with an injectable opener; here the transport wraps reqwest with
//! the same contract: HTTPS-only base URL, Bearer auth, per-request
//! timeout, bounded response reads, `ok=true` + response-type validation,
//! and structured redacted errors for non-success statuses.
//!
//! Divergence note: reqwest reads the body via `chunk()` accumulation with
//! the same `limit + 1` cut as Python's `response.read(limit + 1)`, so the
//! "response exceeded configured size bound" failure is identical while
//! memory stays bounded. Transport-error kinds are reqwest categories
//! ("timeout" / "connect_error" / "transport_error") rather than Python
//! exception class names ("TimeoutError" / "URLError" / ...).
//!
//! The transport body lives beside this entry point: `transport`
//! (construction, the base-URL accessor, and the Skarbiec-backed
//! variant), `endpoint` (Python `_url` and `quote_plus`), `request` (the
//! binary and JSON request verbs over the shared `send`), and `response`
//! (bounded reads, JSON parsing, and the error mapping).

mod endpoint;
mod request;
mod response;
mod transport;

use std::time::Duration;

use super::types::{DEFAULT_BOX_API_URL, DEFAULT_TIMEOUT_SECONDS};

/// Validated transport with no construction-time requests (Python
/// `BoxHTTPTransport`).
#[derive(Debug, Clone)]
pub struct BoxHttpTransport {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    timeout: Duration,
}

/// Default timeout used by [`super::BoxProvider::from_env`] — re-exported
/// for callers that don't go through env resolution.
pub const DEFAULT_TIMEOUT: f64 = DEFAULT_TIMEOUT_SECONDS;
/// Default base URL (re-exported so `super::BoxProvider` mirrors the
/// Python constructor defaults).
pub const DEFAULT_BASE_URL: &str = DEFAULT_BOX_API_URL;
