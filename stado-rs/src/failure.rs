//! Operator-facing failure classification for the `stado` / `wc` CLIs.
//!
//! Every command failure used to reach an operator as one undifferentiated
//! line — `Error: {message}` — and one undifferentiated exit code. "GCS
//! answered 503" and "there is no job 1a2b3c4d" looked identical to the human
//! reading the terminal and, worse, identical to the script wrapping it. A
//! retry loop could not tell which failure it was allowed to retry, so it
//! either hammered a permanent error or gave up on a transient one.
//!
//! This module is the ecosystem failure contract as it applies to a CLI. The
//! code set, the severities, the retryability and the classification rules
//! come from the `wisent-errors` package, which was extracted from this file
//! rather than rewritten, so they are not reinvented here either.
//! Two things are deliberately different here, because the recipient is
//! different:
//!
//! - **No collector call.** A CLI that phones an analytics endpoint on the
//!   failure path acquires a second way to hang, at the exact moment the
//!   network is already suspect. The record is one structured log line on
//!   stderr, carrying `failure_point`, `error_code`, `service` and
//!   `retryable`, and the operator's log shipper does the rest.
//! - **Nothing is hidden.** The contract's rule about withholding exception
//!   text, upstream bodies, environment-variable names and paths protects a
//!   caller reached *over the network*. The operator at this terminal is the
//!   person who has to fix it: the original message keeps being printed in
//!   full, and the technical detail is repeated in the log line.
//!
//! What the contract still buys us here is rule one — an infrastructure
//! failure is never dressed up as a missing resource, and never as success.

use std::sync::LazyLock;

use regex::Regex;

/// The vocabulary and everything derivable from a code come from the fleet
/// package. `wisent-errors` was extracted from this module verbatim: the code
/// set, the severity per code, the retryable and outage sets, the
/// upstream-status classification and the exit-code remap are the same values
/// this file used to spell out, and are now spelled out once for every
/// language. The local names are re-exported so no caller in this crate
/// changes.
pub use wisent_errors::{Code as FailureCode, Severity};

/// The technical detail carried into the structured log line is bounded: an
/// upstream body pasted whole turns one failure into an unreadable log page.
/// The operator still has the unbounded original on the line above it.
///
/// This bound stays local and stays tighter than the package's envelope bound:
/// it governs the CLI's own log line, which the fleet's shippers have been
/// ingesting at this width.
fn max_detail_chars() -> usize {
    "300".parse().expect("valid detail bound")
}

/// `EX_UNAVAILABLE`. The single fleet-wide signal for "this failure is worth
/// retrying later", ratified across every Wisent CLI: a script branches on
/// this one code instead of pattern-matching prose.
///
/// It is deliberately keyed to [`FailureCode::retryable`] and not to
/// [`FailureCode::outage`]. A rate limit is ours to wait out exactly like a
/// dead dependency is, while a broken configuration is our outage but retrying
/// it forever fixes nothing.
pub fn retry_exit_code() -> i32 {
    FailureCode::RETRY_EXIT
}

/// A missing or malformed environment variable is our outage, not the
/// operator's mistake — and neither is a tool we never installed on the box.
const CONFIG_NEEDLES: &[&str] = &[
    "is required",
    "not configured",
    "is not configured",
    "missing env",
    "must be set",
    "env var",
    "command not found",
    "executable file not found",
];

const AUTH_NEEDLES: &[&str] = &[
    "authentication failed",
    "unauthorized",
    "not authorized",
    "permission denied",
    "forbidden",
    "access denied",
    "invalid credentials",
    "invalid token",
    "expired token",
    "token expired",
];

const RATE_LIMIT_NEEDLES: &[&str] = &[
    "rate limit",
    "rate-limit",
    "ratelimit",
    "too many requests",
    "quota exceeded",
    "throttl",
    "retry after",
];

const TIMEOUT_NEEDLES: &[&str] = &[
    "timed out",
    "timeout",
    "deadline has elapsed",
    "deadline exceeded",
    "operation was cancelled",
];

/// Transport-level failures. Every HTTP client words these differently, and
/// the wording is all we get once the error has been flattened to a string.
const NETWORK_NEEDLES: &[&str] = &[
    "error sending request",
    "connection refused",
    "connection reset",
    "connection closed",
    "broken pipe",
    "tcp connect error",
    "dns error",
    "no route to host",
    "network is unreachable",
    "temporary failure in name resolution",
    "econnrefused",
    "enotfound",
    "eai_again",
    "econnreset",
    "socket hang up",
    "service unavailable",
    "bad gateway",
];

/// A resource the operator named that does not exist. Checked last among the
/// needles: "not found" is a substring of far too many sentences that are
/// really about something else, `command not found` being the worst of them.
const NOT_FOUND_NEEDLES: &[&str] = &[
    "not found",
    "no such",
    "does not exist",
    "unknown job",
    "already gone",
];

/// Most of this fleet's failures arrive as prose that has an HTTP status
/// embedded in it — `GCS API error HTTP 503: ...`, `... -> HTTP 429: ...`.
/// That status is real structured evidence and beats any keyword.
static UPSTREAM_STATUS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\bhttp(?:/\d(?:\.\d)?)?\s*(?:status\s*(?:code)?)?\s*[:=]?\s*(?P<status>\d{3})\b",
    )
    .expect("static regex compiles")
});

fn matches_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

/// Bounded technical detail for the structured log line. The width is this
/// CLI's own; the cut is the package's, so the fleet has one trim rule.
pub fn bounded_detail(text: &str) -> String {
    wisent_errors::trim_detail(text, max_detail_chars())
}

/// The status an upstream answered with, if the message names one.
fn upstream_status(message: &str) -> Option<FailureCode> {
    let status = UPSTREAM_STATUS_RE
        .captures(message)?
        .name("status")?
        .as_str()
        .parse::<u16>()
        .ok()?;
    match FailureCode::from_upstream_status(status) {
        // A status we cannot place is no evidence at all; fall through to the
        // wording rather than pinning the failure on a number we misread.
        FailureCode::Unknown => None,
        code => Some(code),
    }
}

/// Classify a command failure from the only thing a flat `CmdError` carries:
/// its message.
///
/// Order matters, and it encodes the contract's priorities. Structured
/// evidence (an upstream status) wins outright. Then our own broken
/// configuration, because that is an outage the operator must be told about
/// even when the sentence also mentions something missing. `not_found` is
/// evaluated last so that a dependency being down can never be reported as a
/// resource being absent.
pub fn classify_message(message: &str) -> FailureCode {
    if let Some(code) = upstream_status(message) {
        return code;
    }
    let haystack = message.to_lowercase();
    if matches_any(&haystack, CONFIG_NEEDLES) {
        return FailureCode::Config;
    }
    if matches_any(&haystack, AUTH_NEEDLES) {
        return FailureCode::Auth;
    }
    if matches_any(&haystack, RATE_LIMIT_NEEDLES) {
        return FailureCode::RateLimit;
    }
    if matches_any(&haystack, TIMEOUT_NEEDLES) {
        return FailureCode::Timeout;
    }
    if matches_any(&haystack, NETWORK_NEEDLES) {
        return FailureCode::InfraDown;
    }
    if matches_any(&haystack, NOT_FOUND_NEEDLES) {
        return FailureCode::NotFound;
    }
    FailureCode::Unknown
}

/// The sentence printed under `Error: ...` — what happened, its code, and
/// whether running the command again can help.
pub fn operator_line(code: FailureCode) -> String {
    let side = if code.outage() {
        "our failure"
    } else {
        "your request or credentials"
    };
    let retry = if code.retryable() {
        "retry later"
    } else {
        "retrying will not help"
    };
    format!(
        "{summary} — {side} [{code}]; {retry}",
        summary = code.operator_summary(),
    )
}

/// The one structured line a log shipper reads. Field names are the
/// ecosystem's: `failure_point`, `error_code`, `service`, `retryable`.
///
/// This is the whole reporting mechanism for a CLI — there is no network call
/// on this path on purpose.
pub fn log_failure(point: &str, service: &str, code: FailureCode, detail: &str) {
    tracing::error!(
        failure_point = point,
        error_code = code.as_str(),
        service = service,
        retryable = code.retryable(),
        severity = code.severity().as_str(),
        detail = %bounded_detail(detail),
        "{}",
        code.operator_summary()
    );
}

