//! The rollout phase as the fleet spells it, and the question of whether a
//! candidate is moving through it right now.

use crate::release_agent::RolloutPhase;

use super::super::constants::PHASE_UNREPORTED;

/// The word [`RolloutPhase`] publishes for itself, so the phase this command
/// prints is the phase the state file and the published status row spell.
/// Every phase is a unit variant renamed to snake case, so `json!` renders
/// it as a string; a variant that ever serialized otherwise would be a
/// phase the status row cannot spell, and is printed as unreported.
pub(super) fn phase_word(phase: RolloutPhase) -> String {
    match serde_json::json!(phase).as_str() {
        Some(word) => word.to_string(),
        None => PHASE_UNREPORTED.to_string(),
    }
}

/// Is a candidate staged, started or under observation right now?
///
/// Enumerated rather than expressed as "not one of the terminal phases":
/// a phase added later must be classified deliberately, not inherit
/// "rolling" from a negation and make a stuck rollout read as one in flight.
pub(super) fn phase_is_rolling(phase: RolloutPhase) -> bool {
    matches!(
        phase,
        RolloutPhase::Downloaded
            | RolloutPhase::Verified
            | RolloutPhase::Staged
            | RolloutPhase::CandidateRunning
            | RolloutPhase::Ready
            | RolloutPhase::Routed
            | RolloutPhase::Monitoring
    )
}
