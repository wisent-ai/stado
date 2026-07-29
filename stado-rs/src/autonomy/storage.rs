//! Canonical object layout and atomic persistence for the autonomy control plane.

use chrono::{DateTime, Duration, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::model::{
    AdoptionRecord, DecisionRecord, InventorySnapshot, SavingsMeasurement, SavingsRecord,
    SCHEMA_VERSION,
};
use super::policy::AutonomyPolicy;
use crate::queue::{JobStorage, StorageError};

pub const POLICY_PATH: &str = "autonomy/policy.json";
pub const INVENTORY_LATEST_PATH: &str = "autonomy/inventory/latest.json";
pub const CONTROL_PATH: &str = "autonomy/control.json";
const INVENTORY_PREFIX: &str = "autonomy/inventory/snapshots";
const DECISION_PREFIX: &str = "autonomy/decisions";
const LEASE_PREFIX: &str = "autonomy/leases";
const SAVINGS_MEASUREMENT_PREFIX: &str = "autonomy/savings-measurements";
const SAVINGS_PREFIX: &str = "autonomy/savings";
const ADOPTION_PREFIX: &str = "autonomy/adoptions";
const FEEDBACK_PREFIX: &str = "autonomy/feedback";

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementLease {
    pub schema_version: u16,
    pub subject_id: String,
    pub decision_id: String,
    pub token: String,
    pub holder: String,
    pub acquired_at: String,
    pub expires_at: String,
}

impl PlacementLease {
    pub fn active_at(&self, now: DateTime<Utc>) -> bool {
        DateTime::parse_from_rfc3339(&self.expires_at)
            .map(|stamp| stamp.with_timezone(&Utc) > now)
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacementFeedback {
    pub schema_version: u16,
    pub decision_id: String,
    pub subject_id: String,
    pub target_id: String,
    pub observed_at: String,
    pub startup_seconds: Option<f64>,
    pub runtime_seconds: Option<f64>,
    pub realized_cost_usd: Option<f64>,
    pub succeeded: bool,
    pub failure_class: Option<String>,
}

pub async fn write_json<T: Serialize>(
    store: &JobStorage,
    path: &str,
    value: &T,
    immutable: bool,
) -> Result<(), StorageError> {
    let content = serde_json::to_string(value)?;
    if !immutable {
        return store.upload_text(path, &content).await;
    }
    if store.create_text_if_absent(path, &content).await? {
        Ok(())
    } else {
        Err(StorageError::StorageConflict(format!(
            "immutable autonomy object already exists: {path}"
        )))
    }
}

pub async fn read_json<T: DeserializeOwned>(
    store: &JobStorage,
    path: &str,
) -> Result<Option<T>, StorageError> {
    let Some(content) = store.download_text(path).await? else {
        return Ok(None);
    };
    Ok(Some(serde_json::from_str(&content)?))
}

pub async fn load_policy(store: &JobStorage) -> Result<AutonomyPolicy, StorageError> {
    let Some(raw) = store.download_text(POLICY_PATH).await? else {
        return Ok(AutonomyPolicy::default());
    };
    let policy: AutonomyPolicy = serde_json::from_str(&raw)?;
    policy.validate().map_err(StorageError::Other)?;
    Ok(policy)
}

pub async fn write_policy(
    store: &JobStorage,
    policy: &AutonomyPolicy,
    expected_version: Option<&str>,
) -> Result<String, StorageError> {
    policy.validate().map_err(StorageError::Other)?;
    let content = serde_json::to_string(policy)?;
    match expected_version {
        Some(version) => {
            store
                .compare_and_swap_text(POLICY_PATH, version, &content)
                .await
        }
        None => {
            if store.create_text_if_absent(POLICY_PATH, &content).await? {
                let stored = store
                    .read_text_versioned(POLICY_PATH)
                    .await?
                    .ok_or_else(|| StorageError::NotFound(POLICY_PATH.to_string()))?;
                Ok(stored.version)
            } else {
                Err(StorageError::StorageConflict(
                    "autonomy policy already exists; expected_version is required".to_string(),
                ))
            }
        }
    }
}

pub async fn load_policy_versioned(
    store: &JobStorage,
) -> Result<Option<crate::queue::VersionedText>, StorageError> {
    store.read_text_versioned(POLICY_PATH).await
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

pub async fn publish_inventory(
    store: &JobStorage,
    snapshot: &InventorySnapshot,
) -> Result<(), StorageError> {
    let content = serde_json::to_string(snapshot)?;
    let path = format!("{INVENTORY_PREFIX}/{}.json", snapshot.snapshot_id);
    if !store.create_text_if_absent(&path, &content).await? {
        return Err(StorageError::StorageConflict(format!(
            "inventory snapshot {} already exists",
            snapshot.snapshot_id
        )));
    }
    store.upload_text(INVENTORY_LATEST_PATH, &content).await
}

pub async fn load_latest_inventory(
    store: &JobStorage,
) -> Result<Option<InventorySnapshot>, StorageError> {
    let Some(raw) = store.download_text(INVENTORY_LATEST_PATH).await? else {
        return Ok(None);
    };
    Ok(Some(serde_json::from_str(&raw)?))
}

pub async fn write_decision(
    store: &JobStorage,
    decision: &DecisionRecord,
) -> Result<(), StorageError> {
    let path = decision_path(&decision.decision_id);
    let content = serde_json::to_string(decision)?;
    if store.create_text_if_absent(&path, &content).await? {
        Ok(())
    } else {
        Err(StorageError::StorageConflict(format!(
            "decision {} already exists",
            decision.decision_id
        )))
    }
}

pub async fn update_decision(
    store: &JobStorage,
    decision: &DecisionRecord,
) -> Result<(), StorageError> {
    let path = decision_path(&decision.decision_id);
    let current = store
        .read_text_versioned(&path)
        .await?
        .ok_or_else(|| StorageError::NotFound(path.clone()))?;
    store
        .compare_and_swap_text(&path, &current.version, &serde_json::to_string(decision)?)
        .await?;
    Ok(())
}

pub async fn load_decision(
    store: &JobStorage,
    decision_id: &str,
) -> Result<Option<DecisionRecord>, StorageError> {
    let Some(raw) = store.download_text(&decision_path(decision_id)).await? else {
        return Ok(None);
    };
    Ok(Some(serde_json::from_str(&raw)?))
}

pub async fn list_decisions(store: &JobStorage) -> Result<Vec<DecisionRecord>, StorageError> {
    load_records(store, &format!("{DECISION_PREFIX}/")).await
}

pub async fn acquire_placement_lease(
    store: &JobStorage,
    subject_id: &str,
    decision_id: &str,
    holder: &str,
    ttl_seconds: u64,
    now: DateTime<Utc>,
) -> Result<Option<PlacementLease>, StorageError> {
    let ttl_seconds = i64::try_from(ttl_seconds)
        .map_err(|_| StorageError::Other("placement lease TTL exceeds i64".to_string()))?;
    let path = lease_path(subject_id);
    let lease = PlacementLease {
        schema_version: SCHEMA_VERSION,
        subject_id: subject_id.to_string(),
        decision_id: decision_id.to_string(),
        token: uuid::Uuid::new_v4().to_string(),
        holder: holder.to_string(),
        acquired_at: now.to_rfc3339(),
        expires_at: (now + Duration::seconds(ttl_seconds)).to_rfc3339(),
    };
    let content = serde_json::to_string(&lease)?;
    if store.create_text_if_absent(&path, &content).await? {
        return Ok(Some(lease));
    }
    let Some(current) = store.read_text_versioned(&path).await? else {
        return Ok(None);
    };
    let prior: PlacementLease = serde_json::from_str(&current.content)?;
    if prior.schema_version != SCHEMA_VERSION {
        return Err(StorageError::Other(format!(
            "unsupported placement lease schema_version {}",
            prior.schema_version
        )));
    }
    if prior.active_at(now) {
        return Ok(None);
    }
    match store
        .compare_and_swap_text(&path, &current.version, &content)
        .await
    {
        Ok(_) => Ok(Some(lease)),
        Err(StorageError::StorageConflict(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

pub async fn release_placement_lease(
    store: &JobStorage,
    subject_id: &str,
    token: &str,
) -> Result<bool, StorageError> {
    let path = lease_path(subject_id);
    let Some(current) = store.read_text_versioned(&path).await? else {
        return Ok(false);
    };
    let lease: PlacementLease = serde_json::from_str(&current.content)?;
    if lease.schema_version != SCHEMA_VERSION {
        return Err(StorageError::Other(format!(
            "unsupported placement lease schema_version {}",
            lease.schema_version
        )));
    }
    if lease.token != token {
        return Ok(false);
    }
    store.delete_blob(&path).await?;
    Ok(true)
}

pub async fn write_feedback(
    store: &JobStorage,
    feedback: &PlacementFeedback,
) -> Result<(), StorageError> {
    let path = format!("{FEEDBACK_PREFIX}/{}.json", feedback.decision_id);
    write_json(store, &path, feedback, true).await
}

pub async fn list_feedback(store: &JobStorage) -> Result<Vec<PlacementFeedback>, StorageError> {
    load_records(store, &format!("{FEEDBACK_PREFIX}/")).await
}

pub async fn write_savings(
    store: &JobStorage,
    savings: &SavingsRecord,
) -> Result<(), StorageError> {
    write_json(
        store,
        &format!("{SAVINGS_PREFIX}/{}.json", savings.savings_id),
        savings,
        true,
    )
    .await
}

pub async fn list_savings(store: &JobStorage) -> Result<Vec<SavingsRecord>, StorageError> {
    load_records(store, &format!("{SAVINGS_PREFIX}/")).await
}
pub async fn write_savings_measurement(
    store: &JobStorage,
    measurement: &SavingsMeasurement,
) -> Result<(), StorageError> {
    write_json(
        store,
        &format!(
            "{SAVINGS_MEASUREMENT_PREFIX}/{}.json",
            measurement.measurement_id
        ),
        measurement,
        true,
    )
    .await
}

pub async fn list_savings_measurements(
    store: &JobStorage,
) -> Result<Vec<SavingsMeasurement>, StorageError> {
    load_records(store, &format!("{SAVINGS_MEASUREMENT_PREFIX}/")).await
}

pub async fn write_adoption(
    store: &JobStorage,
    adoption: &AdoptionRecord,
) -> Result<(), StorageError> {
    let key = hex::encode(Sha256::digest(adoption.resource_id.as_bytes()));
    let path = format!("{ADOPTION_PREFIX}/{key}.json");
    write_json(store, &path, adoption, true).await
}

pub async fn list_adoptions(store: &JobStorage) -> Result<Vec<AdoptionRecord>, StorageError> {
    load_records(store, &format!("{ADOPTION_PREFIX}/")).await
}

async fn load_records<T>(store: &JobStorage, prefix: &str) -> Result<Vec<T>, StorageError>
where
    T: for<'de> Deserialize<'de>,
{
    let mut records = Vec::new();
    for path in store.list_paths(prefix, usize::default()).await? {
        let Some(raw) = store.download_text(&path).await? else {
            continue;
        };
        records.push(serde_json::from_str(&raw)?);
    }
    Ok(records)
}

fn decision_path(decision_id: &str) -> String {
    format!("{DECISION_PREFIX}/{decision_id}.json")
}

fn lease_path(subject_id: &str) -> String {
    let key = hex::encode(Sha256::digest(subject_id.as_bytes()));
    format!("{LEASE_PREFIX}/{key}.json")
}
