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
//! A release digest quarantined for `readiness_probe_unanswered` — the stable
//! bind not answering `/readyz` within three seconds on a host that was out of
//! disk — would never be retried. Days later the agent would still report
//! `phase: quarantined`, `observed: -`, and every command that resolves the
//! release-controlled Skarbiec binary would refuse with `no observed active
//! release (phase Quarantined)`: `credentials item show`, `credentials grant
//! show`, `release catalog declare-publisher`, and with them provider
//! credential reads and the whole fleet's release publication.
//!
//! # What is defended here
//!
//! The agent retires such a record itself, writes its own line in the same
//! audit trail an operator's clear writes, and rolls the desired digest out
//! again. Two bounds keep that from becoming a respawn loop: only a cause that
//! does not hold the candidate is retired at all, and a digest the agent
//! already retried is retried again only once the host has more room than it
//! had at every earlier retry of it — more memory a new allocation can obtain,
//! or more free space on the state directory's volume. A retry that failed
//! with that much room says the candidate needs more; nothing but more room
//! can change its answer, however long the agent waits. Each retry's reading
//! is read back from the audit trail rather than from memory the process does
//! not keep.

use std::io::Write;

use chrono::{DateTime, Utc};
use serde_json::json;

use crate::release_agent::state::document::quarantine_audit_path;
use crate::release_agent::state::records::{HostReleaseState, QuarantineRecord};
use crate::release_cause::QuarantineCause;

mod room;

pub use room::HostRoom;

/// The actor a record written by this agent carries, beside the `$USER` an
/// operator's `quarantine clear` records.
pub const AGENT_ACTOR: &str = "release-agent";

/// Whether the agent may retire one quarantine by itself, and if not, why not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetireVerdict {
    /// The cause says nothing about the candidate; retire the record and let
    /// the rollout try again.
    Retire(QuarantineCause),
    /// The cause is a statement about the release itself. Only the audited
    /// operator command clears it.
    CandidateHeld(QuarantineCause),
    /// This digest was already retried automatically, and the host has no
    /// more room now than it had at the best of those retries.
    Waiting {
        retired_at: DateTime<Utc>,
        best: HostRoom,
        now: HostRoom,
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
            Self::Waiting {
                retired_at,
                best,
                now,
            } => format!(
                "desired release digest is quarantined on this host; the agent last retried it \
                 at {}, and its retries failed with up to {}; it retries once the host has more \
                 of either (now {})",
                retired_at.to_rfc3339(),
                best.described(),
                now.described()
            ),
        }
    }
}

/// The decision itself, with the audit trail's answer already in hand:
/// `previous` is when this digest was last retired automatically and the
/// most room any of its retirements had, `now` the host's room this pass.
///
/// Record interpretation is independent of file I/O, which remains with
/// the caller.
pub fn retire_verdict(
    record: &QuarantineRecord,
    previous: Option<(DateTime<Utc>, HostRoom)>,
    now: HostRoom,
) -> RetireVerdict {
    let cause = record.classification().cause;
    if cause.holds_the_candidate() {
        return RetireVerdict::CandidateHeld(cause);
    }
    if let Some((retired_at, best)) = previous {
        if !now.exceeds(&best) {
            return RetireVerdict::Waiting {
                retired_at,
                best,
                now,
            };
        }
    }
    RetireVerdict::Retire(cause)
}

/// When this agent last retired that exact digest, and the most room any of
/// its retirements of it recorded, read from the audit trail.
///
/// The trail, not a field in the state document: the state document is parsed
/// with `deny_unknown_fields` by every Stado on the fleet, and this fleet
/// demonstrably runs several versions at once, so a new field there would make
/// an older binary treat the whole rollout state as unreadable. The audit file
/// is append-only JSONL that nothing parses strictly, and it already holds the
/// operator's own retirements.
///
/// An unreadable or malformed trail answers `None`, and so do retirements
/// that recorded no reading (written before readings were kept): a
/// retirement that cannot be compared is not a retry the host must beat, and
/// the bound's purpose is to stop futile retries, not to block recovery on a
/// file that says nothing.
pub fn last_auto_retirement(
    state_dir: &str,
    product: &str,
    digest: &str,
) -> Option<(DateTime<Utc>, HostRoom)> {
    let path = quarantine_audit_path(state_dir, product);
    let payload = std::fs::read_to_string(path).ok()?;
    let retirements: Vec<(DateTime<Utc>, HostRoom)> = payload
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|entry| {
            entry["actor"] == AGENT_ACTOR
                && entry["digest"] == digest
                && entry["product"] == product
        })
        .filter_map(|entry| {
            let retired_at = entry["audited_at"]
                .as_str()
                .and_then(|stamp| DateTime::parse_from_rfc3339(stamp).ok())?
                .with_timezone(&Utc);
            let room = HostRoom {
                available_memory_bytes: entry["available_memory_bytes"].as_i64(),
                free_disk_bytes: entry["free_disk_bytes"].as_i64(),
            };
            Some((retired_at, room))
        })
        .collect();
    let latest = retirements.iter().map(|(retired_at, _)| *retired_at).max()?;
    let best = retirements
        .into_iter()
        .map(|(_, room)| room)
        .reduce(HostRoom::widest)?;
    (best != HostRoom::default()).then_some((latest, best))
}

/// Append this agent's own retirement to the trail an operator's clear writes.
///
/// The record carries the quarantine's reason and stamp because retiring the
/// entry deletes both from the state document, and an account that destroys
/// the evidence for the change it documents is decoration. It carries the
/// host's room at the retry too, which the next retirement of this digest
/// must exceed.
#[allow(clippy::too_many_arguments)]
fn record_retirement(
    state_dir: &str,
    product: &str,
    target_name: &str,
    digest: &str,
    record: &QuarantineRecord,
    cause: QuarantineCause,
    audited_at: DateTime<Utc>,
    room: HostRoom,
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
        "available_memory_bytes": room.available_memory_bytes,
        "free_disk_bytes": room.free_disk_bytes,
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
    let room = HostRoom::read(state_dir);
    let verdict = retire_verdict(
        &record,
        last_auto_retirement(state_dir, product, digest),
        room,
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
            room,
        )?;
        state.quarantined.remove(digest);
    }
    Ok(verdict)
}
