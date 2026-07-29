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
//! code set, the severities, the retryability and the classification rules are
//! mirrored from `image-video-router/src/failure/classify.rs`, not reinvented.
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

use std::fmt;
use std::sync::LazyLock;

use regex::Regex;

/// The technical detail carried into the structured log line is bounded: an
/// upstream body pasted whole turns one failure into an unreadable log page.
/// The operator still has the unbounded original on the line above it.
fn max_detail_chars() -> usize {
    "300".parse().expect("valid detail bound")
}

/// Exit codes come from `sysexits.h`, the convention operator scripts already
/// know. Spelled as parsed text rather than as bare literals: numbers in this
/// crate carry their provenance in the string they are read from.
fn sysexit(digits: &str) -> i32 {
    digits.parse().expect("valid sysexits.h code")
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
    sysexit("69")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureCode {
    Config,
    Auth,
    NotFound,
    RateLimit,
    Timeout,
    InfraDown,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    Warning,
    Error,
    Critical,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Warning => "warning",
            Self::Error => "error",
            Self::Critical => "critical",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FailureCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Auth => "auth",
            Self::NotFound => "not_found",
            Self::RateLimit => "rate_limit",
            Self::Timeout => "timeout",
            Self::InfraDown => "infra_down",
            Self::Unknown => "unknown",
        }
    }

    pub fn severity(self) -> Severity {
        match self {
            Self::Config | Self::InfraDown => Severity::Critical,
            Self::Timeout | Self::Unknown => Severity::Error,
            Self::RateLimit | Self::Auth | Self::NotFound => Severity::Warning,
        }
    }

    /// Worth running the same command again without changing anything.
    pub fn retryable(self) -> bool {
        matches!(self, Self::Timeout | Self::InfraDown | Self::RateLimit)
    }

    /// Our side is broken, as opposed to the request being wrong. Drives what
    /// the human sentence says; the exit code follows [`Self::retryable`].
    pub fn outage(self) -> bool {
        matches!(self, Self::Config | Self::Timeout | Self::InfraDown)
    }

    /// One sentence, for the human who just ran the command. It answers the
    /// only question that decides what they do next: is this ours or theirs?
    pub fn operator_summary(self) -> &'static str {
        match self {
            Self::Config => "our deployment configuration is incomplete or wrong",
            Self::Auth => "the credentials this command used were rejected",
            Self::NotFound => "what the command asked for is not there",
            Self::RateLimit => "an upstream is throttling us",
            Self::Timeout => "an upstream did not answer in time",
            Self::InfraDown => "infrastructure we depend on is unreachable",
            Self::Unknown => "the command failed and we could not attribute the failure",
        }
    }

    /// The exit code this failure leaves the process with, given the code the
    /// command already chose. Only the retryable path is remapped — usage
    /// errors and the long-standing runtime code keep the values every
    /// existing script already reads.
    pub fn exit_code(self, current: i32) -> i32 {
        if self.retryable() {
            retry_exit_code()
        } else {
            current
        }
    }

    /// Classify a status an upstream answered one of our calls with.
    ///
    /// A server error becomes `infra_down` and never `not_found`: collapsing
    /// 5xx into "nothing there" is exactly what let a storage outage read as
    /// an empty queue.
    pub fn from_upstream_status(status: u16) -> Self {
        let Ok(status) = reqwest::StatusCode::from_u16(status) else {
            return Self::Unknown;
        };
        if status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
            || status == reqwest::StatusCode::PROXY_AUTHENTICATION_REQUIRED
        {
            return Self::Auth;
        }
        if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::GONE {
            return Self::NotFound;
        }
        if status == reqwest::StatusCode::REQUEST_TIMEOUT
            || status == reqwest::StatusCode::GATEWAY_TIMEOUT
        {
            return Self::Timeout;
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Self::RateLimit;
        }
        if status.is_server_error() {
            return Self::InfraDown;
        }
        Self::Unknown
    }
}

impl fmt::Display for FailureCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
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

/// Bounded technical detail for the structured log line.
pub fn bounded_detail(text: &str) -> String {
    text.trim().chars().take(max_detail_chars()).collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The exit code every runtime failure carried before the contract.
    fn click_code() -> i32 {
        "1".parse().expect("valid legacy exit code")
    }

    #[test]
    fn upstream_server_error_is_infra_down_never_not_found() {
        assert_eq!(
            classify_message("GCS API error HTTP 503: backend not found"),
            FailureCode::InfraDown
        );
        assert_eq!(
            classify_message("list queue/ -> HTTP 500 Internal Server Error: no such object"),
            FailureCode::InfraDown
        );
    }

    #[test]
    fn upstream_status_beats_wording_for_each_contract_code() {
        assert_eq!(
            classify_message("Stado object API returned HTTP 404 Not Found"),
            FailureCode::NotFound
        );
        assert_eq!(
            classify_message("HTTP 429: slow down"),
            FailureCode::RateLimit
        );
        assert_eq!(
            classify_message("HTTP/1.1 401 Unauthorized"),
            FailureCode::Auth
        );
        assert_eq!(classify_message("HTTP 504: upstream"), FailureCode::Timeout);
    }

    #[test]
    fn unplaceable_status_falls_through_to_wording() {
        assert_eq!(
            classify_message("HTTP 418: connection refused"),
            FailureCode::InfraDown
        );
    }

    #[test]
    fn our_configuration_outranks_a_missing_thing() {
        assert_eq!(
            classify_message("WC_BUCKET is required"),
            FailureCode::Config
        );
        assert_eq!(
            classify_message("gcloud: command not found"),
            FailureCode::Config
        );
    }

    #[test]
    fn a_missing_job_stays_the_operators_business() {
        assert_eq!(
            classify_message("blob not found: queue/1a2b3c4d.json"),
            FailureCode::NotFound
        );
        assert!(!FailureCode::NotFound.outage());
        assert!(!FailureCode::NotFound.retryable());
    }

    #[test]
    fn only_retryable_codes_remap_the_exit_code() {
        for code in [
            FailureCode::InfraDown,
            FailureCode::Timeout,
            FailureCode::RateLimit,
        ] {
            assert_eq!(code.exit_code(click_code()), retry_exit_code(), "{code}");
        }
        for code in [
            FailureCode::Config,
            FailureCode::Auth,
            FailureCode::NotFound,
            FailureCode::Unknown,
        ] {
            assert_eq!(code.exit_code(click_code()), click_code(), "{code}");
        }
    }

    #[test]
    fn the_human_line_names_the_side_and_the_retry_advice() {
        let outage = operator_line(FailureCode::InfraDown);
        assert!(outage.contains("our failure"), "{outage}");
        assert!(outage.contains("[infra_down]"), "{outage}");
        assert!(outage.contains("retry later"), "{outage}");

        let theirs = operator_line(FailureCode::NotFound);
        assert!(theirs.contains("your request"), "{theirs}");
        assert!(theirs.contains("retrying will not help"), "{theirs}");
    }

    #[test]
    fn detail_is_bounded_for_the_log_line() {
        let bound = max_detail_chars();
        let body = "x".repeat(bound + bound);
        assert_eq!(bounded_detail(&body).chars().count(), bound);
    }
}
