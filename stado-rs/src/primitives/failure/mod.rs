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
//!
//! A failure's code is what the code that failed stated about it
//! (`CmdError::failure`, a typed error). An error that states none is
//! [`FailureCode::Unknown`]: until 2026-10-01 its code was guessed from the
//! sentence's words against a list of phrases, and the operator ordered every
//! decision by keyword removed. A guess is no evidence; a caller that knows
//! its failure says so where it builds the error.

/// The vocabulary and everything derivable from a code come from the fleet
/// package. `wisent-errors` was extracted from this module verbatim: the code
/// set, the severity per code, the retryable and outage sets, the
/// upstream-status classification and the exit-code remap are the same values
/// this file used to spell out, and are now spelled out once for every
/// language. The local names are re-exported so no caller in this crate
/// changes.
pub use wisent_errors::{Code as FailureCode, Severity};

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
        detail = %detail.trim(),
        "{}",
        code.operator_summary()
    );
}
