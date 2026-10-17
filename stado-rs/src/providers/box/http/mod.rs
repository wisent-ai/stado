//! HTTP transport for Boat Public API v1.
//!
//! reqwest with an HTTPS-only base URL, Bearer auth, whole-body reads,
//! `ok=true` + response-type validation, and structured redacted errors for
//! non-success statuses. A request runs until the API answers.
//!
//! The transport body lives beside this entry point: `transport`
//! (construction, the base-URL accessor, and the Skarbiec-backed variant),
//! `endpoint` (the URL builder), `request` (the binary and JSON request
//! verbs over the shared `send`), and `response` (body reads, JSON parsing,
//! and the error mapping).

mod endpoint;
mod request;
mod response;
mod transport;

use super::types::DEFAULT_BOX_API_URL;

/// Validated transport with no construction-time requests.
#[derive(Debug, Clone)]
pub struct BoxHttpTransport {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
}

/// Boat's public API base, the base `super::BoxProvider` uses when the
/// profile names no other.
pub const DEFAULT_BASE_URL: &str = DEFAULT_BOX_API_URL;
