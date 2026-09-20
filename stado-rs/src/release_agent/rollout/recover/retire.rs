//! Retiring a quarantine the host caused, so a recovered host is not left on
//! a release nobody is coming to re-enable.
//!
//! # The invariant
//!
//! A quarantine stops the agent from respawning a candidate that cannot run.
//! [`QuarantineCause::holds_the_candidate`] already says which refusals are
//! statements about the release and which are statements about the host: a
//! probe the host never answered says nothing about the bytes it could not
//! start. That distinction governed only the *next* candidate
//! ([`super::wall::cause_hold`]); the desired digest itself stayed refused on
//! every pass until a person ran `stado release quarantine clear`.
//!
//! # What went wrong
//!
//! On `lukasz-macbook` the Skarbiec release digest `55f2cf47…` was quarantined
//! at 2026-09-17T21:50:31Z for `readiness_probe_unanswered` — the stable bind
//! did not answer `/readyz` within three seconds on a host that was out of
//! disk. Nothing retried it. Three days later the agent still reported
//! `phase: quarantined`, `observed: -`, and every command that resolves the
//! release-controlled Skarbiec binary refused with `no observed active release
//! (phase Quarantined)`: `credentials item show`, `credentials grant show`,
//! `release catalog declare-publisher`, and with them the Most provider
//! credential read and the whole fleet's release publication.
//!
//! # What is defended here
//!
//! The agent retires such a record itself, writes its own line in the same
//! audit trail an operator's clear writes, and rolls the desired digest out
//! again. Two bounds keep that from becoming a respawn loop: only a cause that
//! does not hold the candidate is retired at all, and a retirement of one
//! digest is not repeated inside [`AUTO_RETIRE_COOLDOWN_SECONDS`], read back
//! from the audit trail rather than from memory the process does not keep.

use std::io::Write;

use chrono::{DateTime, Utc};
use serde_json::json;

use crate::release_agent::state::document::quarantine_audit_path;
use crate::release_agent::state::records::{HostReleaseState, QuarantineRecord};
use crate::release_cause::QuarantineCause;

/// The actor a record written by this agent carries, beside the `$USER` an
/// operator's `quarantine clear` records.
pub const AGENT_ACTOR: &str = "release-agent";

/// How long one digest stays retired-once before the agent may retire it
/// again.
///
/// The bound exists because retiring is a retry: the digest rolls out, and a
/// host that still cannot run it quarantines it again within the readiness
/// window. Without a wait that pair becomes one candidate per tick. An hour is
/// the interval over which a host condition — disk reclaimed, memory returned,
/// load gone — plausibly changes, and it is long enough that a host stuck in
/// the loop spends 24 candidates a day rather than 2880.
pub const AUTO_RETIRE_COOLDOWN_SECONDS: i64 = 3600;

/// Whether the agent may retire one quarantine by itself, and if not, why not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetireVerdict {
    /// The cause says nothing about the candidate; retire the record and let
    /// the rollout try again.
    Retire(QuarantineCause),
    /// The cause is a statement about the release itself. Only the audited
    /// operator command clears it.
    CandidateHeld(QuarantineCause),
    /// This digest was already retried automatically, too recently to learn
    /// anything from retrying it again.
    Cooling {
        retired_at: DateTime<Utc>,
        seconds_left: i64,
    },
}

impl RetireVerdict {
    /// The sentence the state document carries while this verdict holds.
    ///
    /// Written into `detail`, which `release status` and `release doctor`
    /// print verbatim, because "desired release digest is quarantined on this
    /// host" never said whether anything was coming to change that.
    pub fn detail(&self) -> String {
        match self {
            Self::Retire(cause) => format!(
                "desired release digest was quarantined for {}, a host condition; \
                 the agent retired that record and is rolling it out again",
                cause.as_str()
            ),
            Self::CandidateHeld(cause) => format!(
                "desired release digest is quarantined on this host for {}; \
                 it names the candidate, so it is cleared with \
                 stado release quarantine clear --digest <digest> --reason <text>",
                cause.as_str()
            ),
            Self::Cooling {
                retired_at,
                seconds_left,
            } => format!(
                "desired release digest is quarantined on this host; the agent already retried it \
                 at {} and waits {seconds_left}s before retrying again",
                retired_at.to_rfc3339()
            ),
        }
    }
}

/// The decision itself, with the audit trail's answer already in hand.
///
/// Separated from the file reads so every branch is exercised from the state
/// the incident left behind, including the ones a live host will not produce
/// on demand.
pub fn retire_verdict(
    record: &QuarantineRecord,
    last_auto_retirement: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> RetireVerdict {
    let cause = record.classification().cause;
    if cause.holds_the_candidate() {
        return RetireVerdict::CandidateHeld(cause);
    }
    if let Some(retired_at) = last_auto_retirement {
        let elapsed = now.signed_duration_since(retired_at).num_seconds();
        if (0..AUTO_RETIRE_COOLDOWN_SECONDS).contains(&elapsed) {
            return RetireVerdict::Cooling {
                retired_at,
                seconds_left: AUTO_RETIRE_COOLDOWN_SECONDS - elapsed,
            };
        }
    }
    RetireVerdict::Retire(cause)
}

/// When this agent last retired that exact digest, read from the audit trail.
///
/// The trail, not a field in the state document: the state document is parsed
/// with `deny_unknown_fields` by every Stado on the fleet, and this fleet
/// demonstrably runs several versions at once, so a new field there would make
/// an older binary treat the whole rollout state as unreadable. The audit file
/// is append-only JSONL that nothing parses strictly, and it already holds the
/// operator's own retirements.
///
/// An unreadable or malformed trail answers `None`: a retirement that cannot
/// be read is not a retirement that happened, and the cooldown's purpose is to
/// bound retries, not to block recovery on a file that never existed.
pub fn last_auto_retirement(state_dir: &str, product: &str, digest: &str) -> Option<DateTime<Utc>> {
    let path = quarantine_audit_path(state_dir, product);
    let payload = std::fs::read_to_string(path).ok()?;
    payload
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|entry| {
            entry["actor"] == AGENT_ACTOR
                && entry["digest"] == digest
                && entry["product"] == product
        })
        .filter_map(|entry| {
            entry["audited_at"]
                .as_str()
                .and_then(|stamp| DateTime::parse_from_rfc3339(stamp).ok())
                .map(|stamp| stamp.with_timezone(&Utc))
        })
        .max()
}

/// Append this agent's own retirement to the trail an operator's clear writes.
///
/// The record carries the quarantine's reason and stamp because retiring the
/// entry deletes both from the state document, and an account that destroys
/// the evidence for the change it documents is decoration.
fn record_retirement(
    state_dir: &str,
    product: &str,
    target_name: &str,
    digest: &str,
    record: &QuarantineRecord,
    cause: QuarantineCause,
    audited_at: DateTime<Utc>,
) -> Result<(), String> {
    let path = quarantine_audit_path(state_dir, product);
    let mut line = serde_json::to_vec(&json!({
        "actor": AGENT_ACTOR,
        "host": target_name,
        "product": product,
        "digest": digest,
        "reason": format!(
            "{} is a condition of this host, not of the candidate; \
             the agent retries the desired digest",
            cause.as_str()
        ),
        "cause": cause.as_str(),
        "audited_at": audited_at.to_rfc3339(),
        "quarantine_reason": record.reason,
        "quarantined_at": record.quarantined_at.to_rfc3339(),
    }))
    .map_err(|error| format!("cannot encode the quarantine retirement: {error}"))?;
    // A newline inside the record would split one retirement across two rows.
    line.retain(|byte| *byte != b'\n');
    line.push(b'\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| format!("cannot open the quarantine audit trail {path}: {error}"))?;
    file.write_all(&line)
        .map_err(|error| format!("cannot append to the quarantine audit trail {path}: {error}"))
}

/// Retire the desired digest's quarantine when its cause was the host's, and
/// say what was decided either way.
///
/// The state document is left to the caller to save: this runs inside one
/// product's reconcile pass, which already holds that lock and commits the
/// document once.
pub fn retire_host_caused_quarantine(
    state_dir: &str,
    target_name: &str,
    product: &str,
    digest: &str,
    state: &mut HostReleaseState,
) -> Result<RetireVerdict, String> {
    let Some(record) = state.quarantined.get(digest).cloned() else {
        return Err(format!(
            "{product} on {target_name} has no quarantine record for {digest}"
        ));
    };
    let verdict = retire_verdict(
        &record,
        last_auto_retirement(state_dir, product, digest),
        Utc::now(),
    );
    if let RetireVerdict::Retire(cause) = verdict {
        let audited_at = Utc::now();
        record_retirement(
            state_dir,
            product,
            target_name,
            digest,
            &record,
            cause,
            audited_at,
        )?;
        state.quarantined.remove(digest);
    }
    Ok(verdict)
}
