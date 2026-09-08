//! The emergency pause and the mutation circuit breaker: one compare-and-swap
//! object that every mutating stage reads before it acts.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::autonomy::model::SCHEMA_VERSION;
use crate::queue::{JobStorage, StorageError};

use super::CONTROL_PATH;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ControlState {
    pub schema_version: u16,
    pub emergency_paused: bool,
    pub reason: Option<String>,
    pub changed_at: String,
    pub changed_by: String,
    pub consecutive_mutation_failures: usize,
    pub circuit_open_until: Option<String>,
    pub last_mutation_error: Option<String>,
}

impl Default for ControlState {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            emergency_paused: false,
            reason: None,
            changed_at: Utc::now().to_rfc3339(),
            changed_by: "default".to_string(),
            consecutive_mutation_failures: usize::default(),
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
    let state: ControlState = serde_json::from_str(&raw)?;
    if state.schema_version != SCHEMA_VERSION {
        return Err(StorageError::Other(format!(
            "unsupported autonomy control schema_version {}",
            state.schema_version
        )));
    }
    Ok(state)
}

pub async fn set_control(
    store: &JobStorage,
    emergency_paused: bool,
    reason: Option<String>,
    actor: impl Into<String>,
) -> Result<ControlState, StorageError> {
    let actor = actor.into();
    let attempts = (u16::BITS / u8::BITS) as usize;
    for _ in usize::default()..attempts {
        let versioned = store.read_text_versioned(CONTROL_PATH).await?;
        let mut state = match versioned.as_ref() {
            Some(value) => serde_json::from_str::<ControlState>(&value.content)?,
            None => ControlState::default(),
        };
        if state.schema_version != SCHEMA_VERSION {
            return Err(StorageError::Other(format!(
                "unsupported autonomy control schema_version {}",
                state.schema_version
            )));
        }
        state.emergency_paused = emergency_paused;
        state.reason = reason.clone();
        state.changed_at = Utc::now().to_rfc3339();
        state.changed_by = actor.clone();
        let content = serde_json::to_string(&state)?;
        let write = match versioned {
            Some(value) => store
                .compare_and_swap_text(CONTROL_PATH, &value.version, &content)
                .await
                .map(|_| true),
            None => store.create_text_if_absent(CONTROL_PATH, &content).await,
        };
        match write {
            Ok(true) => return Ok(state),
            Ok(false) | Err(StorageError::StorageConflict(_)) => continue,
            Err(error) => return Err(error),
        }
    }
    Err(StorageError::StorageConflict(
        "autonomy control state changed concurrently".to_string(),
    ))
}
pub async fn record_mutation_outcome(
    store: &JobStorage,
    succeeded: bool,
    error: Option<&str>,
    failure_threshold: usize,
    cooldown_seconds: u64,
) -> Result<ControlState, StorageError> {
    if failure_threshold == usize::default() {
        return Err(StorageError::Other(
            "circuit-breaker failure threshold must be positive".to_string(),
        ));
    }
    let cooldown_seconds = i64::try_from(cooldown_seconds)
        .map_err(|_| StorageError::Other("circuit-breaker cooldown exceeds i64".to_string()))?;
    if cooldown_seconds == i64::default() {
        return Err(StorageError::Other(
            "circuit-breaker cooldown must be positive".to_string(),
        ));
    }
    let attempts = (u16::BITS / u8::BITS) as usize;
    for _ in usize::default()..attempts {
        let versioned = store.read_text_versioned(CONTROL_PATH).await?;
        let mut state = match versioned.as_ref() {
            Some(value) => serde_json::from_str::<ControlState>(&value.content)?,
            None => ControlState::default(),
        };
        if state.schema_version != SCHEMA_VERSION {
            return Err(StorageError::Other(format!(
                "unsupported autonomy control schema_version {}",
                state.schema_version
            )));
        }
        if succeeded {
            state.consecutive_mutation_failures = usize::default();
            state.circuit_open_until = None;
            state.last_mutation_error = None;
        } else {
            state.consecutive_mutation_failures = state
                .consecutive_mutation_failures
                .saturating_add(true as usize);
            state.last_mutation_error = error.map(str::to_string);
            if state.consecutive_mutation_failures >= failure_threshold {
                state.circuit_open_until =
                    Some((Utc::now() + Duration::seconds(cooldown_seconds)).to_rfc3339());
            }
        }
        state.changed_at = Utc::now().to_rfc3339();
        state.changed_by = "autonomy-circuit-breaker".to_string();
        let content = serde_json::to_string(&state)?;
        let write = match versioned {
            Some(value) => store
                .compare_and_swap_text(CONTROL_PATH, &value.version, &content)
                .await
                .map(|_| true),
            None => store.create_text_if_absent(CONTROL_PATH, &content).await,
        };
        match write {
            Ok(true) => return Ok(state),
            Ok(false) | Err(StorageError::StorageConflict(_)) => continue,
            Err(error) => return Err(error),
        }
    }
    Err(StorageError::StorageConflict(
        "autonomy circuit-breaker state changed concurrently".to_string(),
    ))
}
