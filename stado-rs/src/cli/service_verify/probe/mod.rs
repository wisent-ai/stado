//! Asking the endpoint, in the language the declaration says it speaks.

mod http;
pub(in crate::cli::service_verify) mod remote;
mod tcp;

use std::time::Duration;

use crate::observations::UNVERIFIED;
use crate::targets::{VERIFY_KIND_HTTP, VERIFY_KIND_TCP};

use crate::cli::service_verify::probe::http::probe_http;
use crate::cli::service_verify::probe::tcp::probe_tcp;

/// A probe must not hang a fleet sweep behind one dead forward. Long enough for
/// a loopback service under load, short enough that a closed laptop answers
/// promptly with the truth.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Ask the endpoint whether anything is there, in the language the declaration
/// says it speaks.
///
/// The catch-all arm is `unverified`, and it is not dead code behind
/// [`unsupported`](super::checks::unsupported): it is the guarantee that no path through this file can
/// turn a kind nobody implemented into a verdict about a service nobody
/// probed.
pub(in crate::cli::service_verify) async fn probe(
    kind: &str,
    endpoint: &str,
) -> (&'static str, String) {
    match kind {
        VERIFY_KIND_HTTP => probe_http(endpoint).await,
        VERIFY_KIND_TCP => probe_tcp(endpoint).await,
        other => (
            UNVERIFIED,
            format!("no probe implemented for verification kind '{other}'"),
        ),
    }
}

/// The operating system's own words, not the wrapper's.
///
/// reqwest reports a dead endpoint as "error sending request for url (...)",
/// which names the URL the caller already knows and hides the one fact worth
/// having: refused, timed out, no route, DNS. Walking to the innermost cause
/// is the difference between a report an operator can act on and one more line
/// that looks like an application bug -- which is exactly how `fetch failed`
/// went unread for twelve days.
fn root_cause(error: &(dyn std::error::Error + 'static)) -> String {
    let mut deepest = error;
    while let Some(source) = deepest.source() {
        deepest = source;
    }
    let message = deepest.to_string();
    let trimmed = message.split(" for url").next().unwrap_or(&message).trim();
    if trimmed.is_empty() {
        return message;
    }
    trimmed.to_string()
}
