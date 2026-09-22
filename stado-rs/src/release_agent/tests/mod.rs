//! The decisions this agent makes about a refusal, exercised from the names
//! the rest of the crate reads them by.

use chrono::{DateTime, Utc};

use super::*;
use crate::release_agent::rollout::recover::run::REPEAT_CAUSE_LIMIT;
use crate::release_agent::state::evidence::clip_middle;
use crate::release_cause::{self, QuarantineCause};

pub(super) fn state_with(quarantines: &[(&str, QuarantineCause, &str)]) -> HostReleaseState {
    let mut state = HostReleaseState::new("brama", "charless-mac-mini");
    for (index, (digest, cause, stamp)) in quarantines.iter().enumerate() {
        state.quarantined.insert(
            (*digest).to_string(),
            QuarantineRecord {
                reason: format!("candidate did not become ready within 90s: reason {index}"),
                quarantined_at: DateTime::parse_from_rfc3339(stamp)
                    .expect("fixture stamp parses")
                    .with_timezone(&Utc),
                cause: *cause,
                evidence: format!("evidence {index}"),
            },
        );
    }
    state
}

/// The digests and stamps are the live `brama` rows from
/// `charless-mac-mini` for 0.2.49, 0.2.50 and 0.2.51 — the three candidates
/// burned inside five hours on 2026-09-01, at the interval this rule is
/// meant for.
///
/// The cause is supplied. On the real host those three wrote no failure
/// line anywhere in their logs and classify as `unclassified`, so no
/// refusal arms there and none should. The run is constructed because the
/// rule has to be exercised somewhere, and inventing the timestamps too
/// would have hidden that the real sequence is this tight.
pub(super) const RUN: &[(&str, QuarantineCause, &str)] = &[
    (
        "217167ef",
        QuarantineCause::CredentialCannotServe,
        "2026-09-01T10:34:46Z",
    ),
    (
        "d862fb1b",
        QuarantineCause::CredentialCannotServe,
        "2026-09-01T10:54:23Z",
    ),
    (
        "4c2bb7c3",
        QuarantineCause::CredentialCannotServe,
        "2026-09-01T15:40:50Z",
    ),
];

/// The product's own decision, with the verdict supplied instead of spawned.
///
/// The decision and the spawn are separated in
/// [`crate::release_agent::rollout::recover::wall::hold_for`] so the decision
/// can be exercised for every verdict, including the ones a real host will
/// not produce on demand — a vault that will not open, a timeout. This calls
/// that function rather than repeating it, so the two cannot drift.
pub(super) fn decide(
    state: &HostReleaseState,
    verdict: Option<(release_cause::WallVerdict, Option<String>)>,
) -> Option<CauseHold> {
    crate::release_agent::rollout::recover::wall::hold_for(cause_run(state)?, verdict)
}

pub(super) const PRESENT: Option<(release_cause::WallVerdict, Option<String>)> =
    Some((release_cause::WallVerdict::Present, None));
pub(super) const GONE: Option<(release_cause::WallVerdict, Option<String>)> =
    Some((release_cause::WallVerdict::Gone, None));


mod binds;
mod holds;
mod records;
