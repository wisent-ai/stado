//! An HTTP endpoint: did anything at all answer?

use crate::observations::{OBSERVED, UNREACHABLE, UNVERIFIED};

use crate::cli::service_verify::probe::{root_cause, PROBE_TIMEOUT};

/// Any HTTP response counts as observed, including 401, 404 and 503. This
/// verifies that the declaration points at something serving, not that the
/// service is healthy: a 503 from a real server is a different and much
/// smaller problem than a connection that goes nowhere, and conflating them is
/// what let `fetch failed` sit in a log for twelve days looking like an
/// application bug.
pub(super) async fn probe_http(url: &str) -> (&'static str, String) {
    let client = match reqwest::Client::builder().timeout(PROBE_TIMEOUT).build() {
        Ok(client) => client,
        Err(error) => return (UNVERIFIED, format!("no HTTP client: {error}")),
    };
    match client.get(url).send().await {
        Ok(response) => (OBSERVED, format!("HTTP {}", response.status().as_u16())),
        Err(error) if error.is_timeout() => (
            UNREACHABLE,
            format!("no answer within {}s", PROBE_TIMEOUT.as_secs()),
        ),
        Err(error) => (UNREACHABLE, root_cause(&error)),
    }
}
