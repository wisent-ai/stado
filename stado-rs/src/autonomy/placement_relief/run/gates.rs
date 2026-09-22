//! What stops a planned move from being made.
//!
//! Five things can, and each one is a different sentence in the report: the
//! policy asked for a report rather than a change, this tick has already spent
//! its relocation, autonomy is paused or its circuit breaker is open, this
//! machine is not the directory authority, or the profile is held by another
//! reconciler. Four of them count as blocked; a report-mode plan does not,
//! because nothing was refused - it was never going to be executed.

use chrono::Utc;

use crate::autonomy::policy::{AutonomyMode, AutonomyPolicy};
use crate::queue::{JobStorage, StorageError};

use super::super::{words, DueAction, MAX_RELOCATIONS_PER_TICK};

/// Why a move is not being made, in the report's own words.
pub(super) struct Refusal {
    pub classification: String,
    pub detail: String,
    /// Whether this counts against the summary's blocked total.
    pub blocked: bool,
}

/// The first reason this move must not be made, or nothing when it may be.
pub(super) async fn refusal(
    store: &JobStorage,
    policy: &AutonomyPolicy,
    authority: &Result<Option<bool>, String>,
    relocated: usize,
    action: &DueAction,
) -> Result<Option<Refusal>, StorageError> {
    if policy.mode == AutonomyMode::Report || policy.emergency_paused {
        let detail = if policy.emergency_paused {
            "mutation blocked by autonomy emergency pause".to_string()
        } else {
            format!(
                "report mode: the {} was planned but not executed",
                match action {
                    DueAction::Move => "move",
                    DueAction::Standby => "standby",
                }
            )
        };
        return Ok(Some(Refusal {
            classification: words::PLANNED.to_string(),
            detail,
            blocked: false,
        }));
    }

    if relocated >= MAX_RELOCATIONS_PER_TICK || relocated >= policy.limits.max_actions_per_tick {
        return Ok(Some(Refusal {
            classification: words::ACTION_LIMIT.to_string(),
            detail: "this tick already spent its relocation".to_string(),
            blocked: true,
        }));
    }

    let control = crate::autonomy::storage::load_control(store).await?;
    if control.emergency_paused || control.circuit_open_at(Utc::now()) {
        return Ok(Some(Refusal {
            classification: words::CONTROL_BLOCKED.to_string(),
            detail: "autonomy pause or circuit breaker became active".to_string(),
            blocked: true,
        }));
    }

    match authority {
        Ok(Some(false)) => Ok(Some(Refusal {
            classification: words::AUTHORITY_ELSEWHERE.to_string(),
            detail: "only the directory authority commits a placement transaction".to_string(),
            blocked: true,
        })),
        Err(error) => Ok(Some(Refusal {
            classification: words::AUTHORITY_ELSEWHERE.to_string(),
            detail: error.clone(),
            blocked: true,
        })),
        Ok(_) => Ok(None),
    }
}
