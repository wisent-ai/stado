//! Durable operation archive and compare-and-swap execution state.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tempfile::NamedTempFile;
use uuid::Uuid;

use crate::queue::{JobStorage, StorageError};

use super::model::{canonical_json_bytes, ActionKind, Plan, SCHEMA_VERSION};
use super::{CmdError, OperationsCommands};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Planned,
    Preflighting,
    Applying,
    Applied,
    ApplyFailed,
    Verifying,
    Verified,
    Drifted,
    Restoring,
    Restored,
    RestoreFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionPhase {
    Pending,
    Preflighted,
    Applying,
    Applied,
    AlreadyDesired,
    Skipped,
    Failed,
    Restoring,
    Restored,
    AlreadyRestored,
    Irreversible,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionState {
    pub action_id: String,
    pub kind: ActionKind,
    pub phase: ActionPhase,
    pub observed_before: Option<Value>,
    pub observed_after: Option<Value>,
    pub receipt: Option<Value>,
    pub error: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationState {
    pub schema_version: u8,
    pub operation_id: String,
    pub plan_hash: String,
    pub phase: Phase,
    pub revision: u64,
    pub created_at: String,
    pub updated_at: String,
    pub actions: BTreeMap<String, ActionState>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationEvent {
    pub schema_version: u8,
    pub event_id: String,
    pub operation_id: String,
    pub recorded_at: String,
    pub event: String,
    pub action_id: Option<String>,
    pub detail: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OperationLease {
    schema_version: u8,
    owner: String,
    acquired_at: String,
    expires_at: String,
}

pub struct Journal {
    store: JobStorage,
    local_root: PathBuf,
}

impl Journal {
    pub async fn open() -> Result<Self, CmdError> {
        let store = JobStorage::new().await?;
        let home = std::env::var_os("HOME").ok_or_else(|| CmdError::click("HOME is not set"))?;
        let local_root = PathBuf::from(home).join(".stado").join("operations");
        Ok(Self { store, local_root })
    }

    pub async fn create(&self, plan: &Plan) -> Result<OperationState, CmdError> {
        plan.validate()?;
        validate_operation_id(&plan.operation_id)?;
        let bytes = plan.canonical_bytes()?;
        let text =
            String::from_utf8(bytes.clone()).map_err(|error| CmdError::click(error.to_string()))?;
        let plan_path = remote_path(&plan.operation_id, "plan.json");
        if !self.store.create_text_if_absent(&plan_path, &text).await? {
            let existing = self.store.download_text(&plan_path).await?.ok_or_else(|| {
                CmdError::click("operation plan disappeared after create conflict")
            })?;
            if existing.as_bytes() != bytes {
                return Err(CmdError::click(format!(
                    "operation {} already has a different immutable plan",
                    plan.operation_id
                )));
            }
        }
        self.write_local(&plan.operation_id, "plan.json", &bytes)?;

        let created_at = now();
        let actions = plan
            .actions
            .iter()
            .map(|action| {
                (
                    action.id.clone(),
                    ActionState {
                        action_id: action.id.clone(),
                        kind: action.kind,
                        phase: ActionPhase::Pending,
                        observed_before: None,
                        observed_after: None,
                        receipt: None,
                        error: None,
                        updated_at: created_at.clone(),
                    },
                )
            })
            .collect();
        let state = OperationState {
            schema_version: SCHEMA_VERSION,
            operation_id: plan.operation_id.clone(),
            plan_hash: plan.sha256()?,
            phase: Phase::Planned,
            revision: u64::default(),
            created_at: created_at.clone(),
            updated_at: created_at,
            actions,
            error: None,
        };
        let body = String::from_utf8(canonical_json_bytes(&state)?)
            .map_err(|error| CmdError::click(error.to_string()))?;
        let state_path = remote_path(&plan.operation_id, "state.json");
        if !self.store.create_text_if_absent(&state_path, &body).await? {
            let existing = self.load_state(&plan.operation_id).await?;
            if existing.plan_hash != state.plan_hash {
                return Err(CmdError::click(
                    "existing operation state references a different plan hash",
                ));
            }
            let existing_bytes = canonical_json_bytes(&existing)?;
            self.write_local(&plan.operation_id, "state.json", &existing_bytes)?;
            return Ok(existing);
        }
        self.write_local(&plan.operation_id, "state.json", body.as_bytes())?;
        self.event(
            &plan.operation_id,
            "planned",
            None,
            json!({"plan_hash": state.plan_hash, "actions": state.actions.len()}),
        )
        .await?;
        Ok(state)
    }

    pub async fn load_plan(&self, operation_id: &str) -> Result<Plan, CmdError> {
        validate_operation_id(operation_id)?;
        let body = self
            .store
            .download_text(&remote_path(operation_id, "plan.json"))
            .await?
            .ok_or_else(|| CmdError::click(format!("operation {operation_id} has no plan")))?;
        let plan: Plan = serde_json::from_str(&body)?;
        plan.validate()?;
        if plan.operation_id != operation_id {
            return Err(CmdError::click("operation id does not match archived plan"));
        }
        if plan.canonical_bytes()? != body.as_bytes() {
            return Err(CmdError::click("archived plan is not canonical Stado JSON"));
        }
        Ok(plan)
    }

    pub async fn load_state(&self, operation_id: &str) -> Result<OperationState, CmdError> {
        validate_operation_id(operation_id)?;
        let body = self
            .store
            .download_text(&remote_path(operation_id, "state.json"))
            .await?
            .ok_or_else(|| CmdError::click(format!("operation {operation_id} has no state")))?;
        let state: OperationState = serde_json::from_str(&body)?;
        validate_state(operation_id, &state)?;
        Ok(state)
    }

    pub async fn update<F>(&self, operation_id: &str, change: F) -> Result<OperationState, CmdError>
    where
        F: FnOnce(&mut OperationState) -> Result<(), CmdError>,
    {
        validate_operation_id(operation_id)?;
        let path = remote_path(operation_id, "state.json");
        let versioned = self
            .store
            .read_text_versioned(&path)
            .await?
            .ok_or_else(|| CmdError::click(format!("operation {operation_id} has no state")))?;
        let mut state: OperationState = serde_json::from_str(&versioned.content)?;
        validate_state(operation_id, &state)?;
        let before_actions = state.actions.clone();
        change(&mut state)?;
        state.revision = state.revision.saturating_add(true as u64);
        let updated_at = now();
        for (action_id, action) in &mut state.actions {
            if before_actions.get(action_id) != Some(action) {
                action.updated_at = updated_at.clone();
            }
        }
        state.updated_at = updated_at;
        let body = String::from_utf8(canonical_json_bytes(&state)?)
            .map_err(|error| CmdError::click(error.to_string()))?;
        self.store
            .compare_and_swap_text(&path, &versioned.version, &body)
            .await
            .map_err(map_conflict)?;
        self.write_local(operation_id, "state.json", body.as_bytes())?;
        Ok(state)
    }

    pub async fn acquire(&self, operation_id: &str) -> Result<String, CmdError> {
        validate_operation_id(operation_id)?;
        let owner = format!(
            "{}-{}",
            crate::watchdog::hostname(),
            Uuid::new_v4().simple()
        );
        let now_value = Utc::now();
        let lease = OperationLease {
            schema_version: SCHEMA_VERSION,
            owner: owner.clone(),
            acquired_at: timestamp(now_value),
            expires_at: timestamp(now_value + lease_duration()),
        };
        let body = String::from_utf8(canonical_json_bytes(&lease)?)
            .map_err(|error| CmdError::click(error.to_string()))?;
        let path = remote_path(operation_id, "lock.json");
        if self.store.create_text_if_absent(&path, &body).await? {
            return Ok(owner);
        }
        let versioned = self
            .store
            .read_text_versioned(&path)
            .await?
            .ok_or_else(|| CmdError::click("operation lock disappeared"))?;
        let current: OperationLease = serde_json::from_str(&versioned.content)?;
        let expires = DateTime::parse_from_rfc3339(&current.expires_at)
            .map_err(|error| CmdError::click(format!("invalid operation lock: {error}")))?;
        if expires.with_timezone(&Utc) > Utc::now() {
            return Err(CmdError::click(format!(
                "operation is locked by {} until {}",
                current.owner, current.expires_at
            )));
        }
        self.store
            .compare_and_swap_text(&path, &versioned.version, &body)
            .await
            .map_err(map_conflict)?;
        Ok(owner)
    }

    pub async fn renew(&self, operation_id: &str, owner: &str) -> Result<(), CmdError> {
        let path = remote_path(operation_id, "lock.json");
        let versioned = self
            .store
            .read_text_versioned(&path)
            .await?
            .ok_or_else(|| CmdError::click("operation lock disappeared"))?;
        let mut lease: OperationLease = serde_json::from_str(&versioned.content)?;
        if lease.owner != owner {
            return Err(CmdError::click(
                "operation lock was lost before the next mutation",
            ));
        }
        lease.expires_at = timestamp(Utc::now() + lease_duration());
        let body = String::from_utf8(canonical_json_bytes(&lease)?)
            .map_err(|error| CmdError::click(error.to_string()))?;
        self.store
            .compare_and_swap_text(&path, &versioned.version, &body)
            .await
            .map_err(map_conflict)?;
        Ok(())
    }

    pub async fn release(&self, operation_id: &str, owner: &str) -> Result<(), CmdError> {
        let path = remote_path(operation_id, "lock.json");
        let Some(versioned) = self.store.read_text_versioned(&path).await? else {
            return Ok(());
        };
        let mut lease: OperationLease = serde_json::from_str(&versioned.content)?;
        if lease.owner != owner {
            return Err(CmdError::click("operation lock ownership changed"));
        }
        lease.expires_at = now();
        let body = String::from_utf8(canonical_json_bytes(&lease)?)
            .map_err(|error| CmdError::click(error.to_string()))?;
        self.store
            .compare_and_swap_text(&path, &versioned.version, &body)
            .await
            .map_err(map_conflict)?;
        Ok(())
    }

    pub async fn event(
        &self,
        operation_id: &str,
        event: &str,
        action_id: Option<&str>,
        detail: Value,
    ) -> Result<(), CmdError> {
        let record = OperationEvent {
            schema_version: SCHEMA_VERSION,
            event_id: Uuid::new_v4().to_string(),
            operation_id: operation_id.to_string(),
            recorded_at: now(),
            event: event.to_string(),
            action_id: action_id.map(str::to_string),
            detail,
        };
        let bytes = canonical_json_bytes(&record)?;
        let body =
            String::from_utf8(bytes.clone()).map_err(|error| CmdError::click(error.to_string()))?;
        let name = format!("events/{}-{}.json", compact_now(), record.event_id);
        let path = remote_path(operation_id, &name);
        if !self.store.create_text_if_absent(&path, &body).await? {
            return Err(CmdError::click("operation event id collision"));
        }
        self.write_local(operation_id, &name, &bytes)?;
        Ok(())
    }

    pub async fn write_artifact<T: Serialize>(
        &self,
        operation_id: &str,
        name: &str,
        value: &T,
    ) -> Result<(), CmdError> {
        validate_artifact_name(name)?;
        let bytes = canonical_json_bytes(value)?;
        let body =
            String::from_utf8(bytes.clone()).map_err(|error| CmdError::click(error.to_string()))?;
        let path = remote_path(operation_id, name);
        self.store.upload_text(&path, &body).await?;
        self.write_local(operation_id, name, &bytes)
    }

    fn write_local(&self, operation_id: &str, name: &str, bytes: &[u8]) -> Result<(), CmdError> {
        validate_operation_id(operation_id)?;
        validate_artifact_name(name)?;
        let path = self.local_root.join(operation_id).join(name);
        atomic_write(&path, bytes)
    }
}

pub async fn dispatch(command: OperationsCommands) -> Result<(), CmdError> {
    let journal = Journal::open().await?;
    match command {
        OperationsCommands::List => list(&journal).await,
        OperationsCommands::Show { operation_id } => show(&journal, &operation_id).await,
    }
}

async fn list(journal: &Journal) -> Result<(), CmdError> {
    let names = journal
        .store
        .list_paths("operations/", usize::default())
        .await?;
    let mut ids = BTreeSet::new();
    for name in names {
        if let Some(rest) = name.strip_prefix("operations/") {
            if let Some((operation_id, _)) = rest.split_once('/') {
                if validate_operation_id(operation_id).is_ok() {
                    ids.insert(operation_id.to_string());
                }
            }
        }
    }
    let mut rows = Vec::new();
    for operation_id in ids.into_iter().rev() {
        match journal.load_state(&operation_id).await {
            Ok(state) => rows.push(json!({
                "operation_id": operation_id,
                "phase": state.phase,
                "plan_hash": state.plan_hash,
                "updated_at": state.updated_at,
                "error": state.error,
            })),
            Err(error) => rows.push(json!({
                "operation_id": operation_id,
                "phase": "unreadable",
                "error": error.to_string(),
            })),
        }
    }
    println!("{}", serde_json::to_string_pretty(&rows)?);
    Ok(())
}

async fn show(journal: &Journal, operation_id: &str) -> Result<(), CmdError> {
    let plan = journal.load_plan(operation_id).await?;
    let state = journal.load_state(operation_id).await?;
    let event_names = journal
        .store
        .list_paths(
            &format!("operations/{operation_id}/events/"),
            usize::default(),
        )
        .await?;
    let mut events = Vec::new();
    for path in event_names {
        if let Some(body) = journal.store.download_text(&path).await? {
            events.push(serde_json::from_str::<Value>(&body)?);
        }
    }
    events.sort_by(|left, right| {
        left["recorded_at"]
            .as_str()
            .cmp(&right["recorded_at"].as_str())
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "operation_id": operation_id,
            "plan": plan,
            "state": state,
            "events": events,
        }))?
    );
    Ok(())
}

fn validate_state(operation_id: &str, state: &OperationState) -> Result<(), CmdError> {
    if state.schema_version != SCHEMA_VERSION || state.operation_id != operation_id {
        return Err(CmdError::click("invalid operation state document"));
    }
    Ok(())
}

fn validate_operation_id(value: &str) -> Result<(), CmdError> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(CmdError::usage(format!("invalid operation id {value:?}")));
    }
    Ok(())
}

fn validate_artifact_name(value: &str) -> Result<(), CmdError> {
    if value.is_empty()
        || value.starts_with('/')
        || value.contains("..")
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'))
    {
        return Err(CmdError::click(format!(
            "invalid operation artifact {value:?}"
        )));
    }
    Ok(())
}

fn remote_path(operation_id: &str, name: &str) -> String {
    format!("operations/{operation_id}/{name}")
}

fn map_conflict(error: StorageError) -> CmdError {
    match error {
        StorageError::StorageConflict(_) => CmdError::click(
            "operation state changed concurrently; inspect it before deciding whether to resume",
        ),
        other => CmdError::click(other.to_string()),
    }
}

fn lease_duration() -> Duration {
    Duration::hours((true as i64).saturating_add(true as i64))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), CmdError> {
    let parent = path
        .parent()
        .ok_or_else(|| CmdError::click(format!("{} has no parent", path.display())))?;
    fs::create_dir_all(parent)?;

    let mut temporary = NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

fn now() -> String {
    timestamp(Utc::now())
}

fn compact_now() -> String {
    Utc::now().format("%Y%m%dT%H%M%S%.fZ").to_string()
}

fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Millis, true)
}
