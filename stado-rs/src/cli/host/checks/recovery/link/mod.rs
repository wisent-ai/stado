//! `stado host link` — why this host went quiet.

pub(in crate::cli::host) mod probe;
pub(in crate::cli::host) mod render;
pub(in crate::cli::host) mod repair;
pub(in crate::cli::host) mod report;

use crate::cli::CmdError;

use crate::cli::host::checks::LINK_HEALTHY;

/// One silence instant, spelled the way the record on disk spells it.
///
/// `AutoSi` and a `Z`, which is what `chrono`'s own serialization writes into
/// the blob: the report is a pointer into `host_silence/<host>/`, and an
/// operator who copies the instant out of this report has to be able to find
/// the record it names. `to_rfc3339`'s `+00:00` would not match it.
fn silence_instant(at: chrono::DateTime<chrono::Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
}

/// `token=count` pairs for one refusal summary, in the stable order the
/// summary's own map holds them.
fn reason_counts(refusals: &crate::monitor::host_silence::RefusalSummary) -> String {
    refusals
        .reasons
        .iter()
        .map(|(reason, count)| format!("{reason}={count}"))
        .collect::<Vec<String>>()
        .join(", ")
}

/// A host whose link is not healthy is a failed verdict, not a failed command:
/// the read succeeded either way.
///
/// The blockers stay in the report and deliberately out of this sentence. They
/// carry the reader's and the channel's own words — "ssh connect Operation
/// timed out" among them — and [`crate::primitives::failure::classify_message`] reads
/// "timed out" in a command's failure message as a retryable failure, which
/// would remap this command's exit status away from the 1 that every
/// non-healthy verdict owes its caller.
fn link_outcome(host: &str, verdict: &str, blockers: usize) -> Result<(), CmdError> {
    if verdict == LINK_HEALTHY {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{host} link verdict is {verdict}, with {blockers} blocker(s) named in the report above"
    )))
}
