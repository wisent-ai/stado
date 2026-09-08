//! The lease state machine: the serialized names and the transition table
//! every fence-gated step is checked against.

use std::str::FromStr;

use super::LeaseError;

/// Python `LeaseState` (str Enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseState {
    Allocating,
    Provisioning,
    Ready,
    Starting,
    Running,
    Collecting,
    Releasing,
    Released,
    Failed,
}

impl LeaseState {
    /// The serialized value (Python `.value`).
    pub fn as_str(self) -> &'static str {
        match self {
            LeaseState::Allocating => "allocating",
            LeaseState::Provisioning => "provisioning",
            LeaseState::Ready => "ready",
            LeaseState::Starting => "starting",
            LeaseState::Running => "running",
            LeaseState::Collecting => "collecting",
            LeaseState::Releasing => "releasing",
            LeaseState::Released => "released",
            LeaseState::Failed => "failed",
        }
    }

    /// Python `_ALLOWED_TRANSITIONS`.
    pub(super) fn allowed_transitions(self) -> &'static [LeaseState] {
        match self {
            LeaseState::Allocating => &[LeaseState::Provisioning, LeaseState::Failed],
            LeaseState::Provisioning => &[LeaseState::Ready, LeaseState::Failed],
            LeaseState::Ready => &[LeaseState::Starting, LeaseState::Failed],
            LeaseState::Starting => &[LeaseState::Running, LeaseState::Failed],
            LeaseState::Running => &[LeaseState::Collecting, LeaseState::Failed],
            LeaseState::Collecting => &[LeaseState::Releasing, LeaseState::Failed],
            LeaseState::Releasing => &[LeaseState::Released, LeaseState::Failed],
            LeaseState::Failed => &[LeaseState::Releasing, LeaseState::Released],
            LeaseState::Released => &[],
        }
    }
}

impl FromStr for LeaseState {
    type Err = LeaseError;

    /// Python `LeaseState(value)` (raises `ValueError` on unknown states).
    fn from_str(value: &str) -> Result<Self, LeaseError> {
        let state = match value {
            "allocating" => LeaseState::Allocating,
            "provisioning" => LeaseState::Provisioning,
            "ready" => LeaseState::Ready,
            "starting" => LeaseState::Starting,
            "running" => LeaseState::Running,
            "collecting" => LeaseState::Collecting,
            "releasing" => LeaseState::Releasing,
            "released" => LeaseState::Released,
            "failed" => LeaseState::Failed,
            other => {
                return Err(LeaseError::Value(format!(
                    "{other:?} is not a valid LeaseState"
                )));
            }
        };
        Ok(state)
    }
}
