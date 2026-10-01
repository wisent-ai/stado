//! The emergency pause and the mutation circuit breaker: one compare-and-swap
//! object that every mutating stage reads before it acts. A write that loses
//! the compare-and-swap is refused as a conflict; the caller decides whether
//! to ask again.

use std::num::{NonZeroU64, NonZeroUsize};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::queue::{JobStorage, StorageError};

use super::CONTROL_PATH;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ControlState {
    pub emergency_paused: bool,
    pub reason: Option<String>,
    pub changed_at: String,
    pub changed_by: String,
    /// When each mutation since the last success failed; the breaker opens
    /// once there are as many as the policy's threshold.
    pub mutation_failures: Vec<String>,
    pub circuit_open_until: Option<String>,
    pub last_mutation_error: Option<String>,
}

impl Default for ControlState {
    fn default() -> Self {
        Self {
            emergency_paused: false,
            reason: None,
            changed_at: Utc::now().to_rfc3339(),
            changed_by: "default".to_string(),
            mutation_failures: Vec::new(),
            circuit_open_until: None,
            last_mutation_error: None,
        }
    }
}
impl ControlState {
    pub fn circuit_open_at(&self, now: DateTime<Utc>) -> bool {
        self.circuit_open_until
            .as_deref()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .is_some_and(|until| until.with_timezone(&Utc) > now)
    }
}

pub async fn load_control(store: &JobStorage) -> Result<ControlState, StorageError> {
    let Some(raw) = store.download_text(CONTROL_PATH).await? else {
        return Ok(ControlState::default());
    };
    Ok(serde_json::from_str(&raw)?)
}

/// Read the state, change it, and write it back against the version read.
async fn update(
    store: &JobStorage,
    conflict: &str,
    change: impl FnOnce(&mut ControlState),
) -> Result<ControlState, StorageError> {
    let versioned = store.read_text_versioned(CONTROL_PATH).await?;
    let mut state = match versioned.as_ref() {
        Some(value) => serde_json::from_str::<ControlState>(&value.content)?,
        None => ControlState::default(),
    };
    change(&mut state);
    let content = serde_json::to_string(&state)?;
    let write = match versioned {
        Some(value) => store
            .compare_and_swap_text(CONTROL_PATH, &value.version, &content)
            .await
            .map(|_| true),
        None => store.create_text_if_absent(CONTROL_PATH, &content).await,
    };
    match write {
        Ok(true) => Ok(state),
        Ok(false) | Err(StorageError::StorageConflict(_)) => {
            Err(StorageError::StorageConflict(conflict.to_string()))
        }
        Err(error) => Err(error),
    }
}

pub async fn set_control(
    store: &JobStorage,
    emergency_paused: bool,
    reason: Option<String>,
    actor: impl Into<String>,
) -> Result<ControlState, StorageError> {
    let actor = actor.into();
    update(
        store,
        "autonomy control state changed concurrently",
        |state| {
            state.emergency_paused = emergency_paused;
            state.reason = reason;
            state.changed_at = Utc::now().to_rfc3339();
            state.changed_by = actor;
        },
    )
    .await
}

pub async fn record_mutation_outcome(
    store: &JobStorage,
    succeeded: bool,
    error: Option<&str>,
    failure_threshold: usize,
    cooldown_seconds: u64,
) -> Result<ControlState, StorageError> {
    let failure_threshold = NonZeroUsize::new(failure_threshold).ok_or_else(|| {
        StorageError::Other("circuit-breaker failure threshold must be positive".to_string())
    })?;
    let cooldown_seconds = NonZeroU64::new(cooldown_seconds)
        .and_then(|value| i64::try_from(value.get()).ok())
        .ok_or_else(|| {
            StorageError::Other(
                "circuit-breaker cooldown must fit positive i64 seconds".to_string(),
            )
        })?;
    update(
        store,
        "autonomy circuit-breaker state changed concurrently",
        |state| {
            let now = Utc::now();
            if succeeded {
                state.mutation_failures.clear();
                state.circuit_open_until = None;
                state.last_mutation_error = None;
            } else {
                state.mutation_failures.push(now.to_rfc3339());
                state.last_mutation_error = error.map(str::to_string);
                if state.mutation_failures.len() >= failure_threshold.get() {
                    state.circuit_open_until =
                        Some((now + Duration::seconds(cooldown_seconds)).to_rfc3339());
                }
            }
            state.changed_at = now.to_rfc3339();
            state.changed_by = "autonomy-circuit-breaker".to_string();
        },
    )
    .await
}
