//! The execution context: the entry points a plan drives, and the VM family
//! itself, whose ownership is re-checked against the fleet audit before an
//! instance is deleted.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::cli::resources::model::{Action, ActionKind, Condition, ProviderKind, Rollback};
use crate::cli::CmdError;
use crate::providers::get_provider;
use crate::queue::JobStorage;

use super::backup::{disable_backup_config, enable_backup_config, inspect_backup_config};
use super::conditions::conditions_match;
use super::gcp::GcpRest;

pub struct Context {
    store: JobStorage,
    gcp: Option<GcpRest>,
}

impl Context {
    pub async fn new(actions: &[Action]) -> Result<Self, CmdError> {
        let store = JobStorage::new().await?;
        let projects: BTreeSet<&str> = actions
            .iter()
            .filter(|action| action.resource.provider == ProviderKind::Gcp)
            .filter_map(|action| action.resource.project.as_deref())
            .collect();
        if projects.len() > true as usize {
            return Err(CmdError::click(
                "one execution batch cannot span multiple GCP projects",
            ));
        }
        let gcp = match projects.first().copied() {
            Some(project) => Some(GcpRest::new(project).await?),
            None => None,
        };
        Ok(Self { store, gcp })
    }

    pub async fn inspect(&self, action: &Action) -> Result<Value, CmdError> {
        match action.kind {
            ActionKind::DeleteInstance => self.inspect_vm(action).await,
            ActionKind::SnapshotDisk | ActionKind::DeleteDisk => {
                self.gcp()?.inspect_disk(action).await
            }
            ActionKind::ReleaseAddress => self.gcp()?.inspect_address(action).await,
            ActionKind::DeleteManagedInstanceGroup | ActionKind::ResizeManagedInstanceGroup => {
                self.gcp()?.inspect_mig(action).await
            }
            ActionKind::ReleaseReservation => self.gcp()?.inspect_reservation(action).await,
            ActionKind::DisableStorageBackup => inspect_backup_config(),
            ActionKind::PauseScheduler => self.gcp()?.inspect_scheduler(action).await,
            ActionKind::StopInstance | ActionKind::StartInstance => self.inspect_vm(action).await,
            ActionKind::SuspendCloudSql => self.gcp()?.inspect_sql(action).await,
            rollback => Err(CmdError::click(format!(
                "rollback-only action {rollback:?} cannot be inspected as a plan action"
            ))),
        }
    }

    pub async fn apply(&self, action: &Action) -> Result<Value, CmdError> {
        match action.kind {
            ActionKind::DeleteInstance => self.delete_vm(action).await,
            ActionKind::SnapshotDisk => self.gcp()?.snapshot_disk(action).await,
            ActionKind::DeleteDisk => self.gcp()?.delete_disk(action).await,
            ActionKind::ReleaseAddress => self.gcp()?.release_address(action).await,
            ActionKind::DeleteManagedInstanceGroup => self.gcp()?.delete_mig(action).await,
            ActionKind::ReleaseReservation => self.gcp()?.release_reservation(action).await,
            ActionKind::DisableStorageBackup => disable_backup_config(action),
            ActionKind::PauseScheduler => self.gcp()?.pause_scheduler(action).await,
            ActionKind::ResizeManagedInstanceGroup => self.gcp()?.resize_mig(action).await,
            ActionKind::StopInstance => self.stop_vm(action).await,
            ActionKind::StartInstance => self.start_vm(action).await,
            ActionKind::SuspendCloudSql => self.gcp()?.suspend_sql(action).await,
            rollback => Err(CmdError::click(format!(
                "rollback-only action {rollback:?} cannot be applied directly"
            ))),
        }
    }

    pub async fn restore(
        &self,
        action: &Action,
        rollback: &Rollback,
        receipt: Option<&Value>,
    ) -> Result<Value, CmdError> {
        match rollback.kind {
            ActionKind::DeleteSnapshot => self.gcp()?.delete_snapshot(action, rollback).await,
            ActionKind::RestoreDisk => self.gcp()?.restore_disk(action, rollback).await,
            ActionKind::EnableStorageBackup => enable_backup_config(action, receipt),
            ActionKind::ResumeScheduler => self.gcp()?.resume_scheduler(action).await,
            ActionKind::ResizeManagedInstanceGroup => {
                self.gcp()?
                    .resize_mig_with(action, &rollback.parameters)
                    .await
            }
            ActionKind::StartInstance => self.start_vm(action).await,
            ActionKind::StopInstance => self.stop_vm(action).await,
            ActionKind::RestoreCloudSql => self.gcp()?.restore_sql(action, rollback).await,
            kind => Err(CmdError::click(format!(
                "action {} has unsupported rollback kind {kind:?}",
                action.id
            ))),
        }
    }

    pub async fn wait_for(
        &self,
        action: &Action,
        conditions: &[Condition],
    ) -> Result<Value, CmdError> {
        let timeout = Duration::from_secs(
            chrono::Duration::minutes(true as i64)
                .num_seconds()
                .try_into()
                .unwrap_or_default(),
        );
        let deadline = Instant::now() + timeout;
        loop {
            let observed = self.inspect(action).await?;
            if conditions_match(conditions, &observed) {
                return Ok(observed);
            }
            if Instant::now() >= deadline {
                return Err(CmdError::click(format!(
                    "postconditions did not converge for {}: observed {}",
                    action.resource.reference, observed
                )));
            }
            tokio::time::sleep(Duration::from_secs(true as u64)).await;
        }
    }

    async fn inspect_vm(&self, action: &Action) -> Result<Value, CmdError> {
        let provider = provider_name(action.resource.provider)?;
        let providers = vec![provider.to_string()];
        let fleet = crate::cli::instances::audit_inventory(&self.store, &providers).await?;
        if let Some(error) = fleet.errors.get(provider) {
            return Err(CmdError::click(format!(
                "cannot inspect {provider} ownership: {error}"
            )));
        }
        if let Some(row) = fleet
            .rows
            .iter()
            .find(|row| row.reference == action.resource.reference)
        {
            let inventory_context = self.inventory_context(action).await?;
            let inventory_orphan = inventory_context.map(|(orphan, _)| orphan).unwrap_or(true);
            let age_seconds = inventory_context
                .map(|(_, age_seconds)| age_seconds as f64)
                .unwrap_or(row.age_seconds);
            return Ok(json!({
                "exists": true,
                "running": true,
                "stopped": false,
                "lifecycle_state": "running",
                "orphan": row.held_by.is_empty() && inventory_orphan,
                "age_seconds": age_seconds,
                "held_by": row.held_by,
            }));
        }
        let client = get_provider(provider)?;
        let lifecycle = client
            .instance_lifecycle_state(&action.resource.reference)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        let exists = client
            .instance_exists(&action.resource.reference)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        let normalized = lifecycle
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let stopped = matches!(
            normalized.as_str(),
            "stopped" | "stopping" | "deallocated" | "deallocating"
        ) || (provider == "gcp" && normalized == "terminated");
        let present = exists || stopped;
        let inventory_context = self.inventory_context(action).await?;
        let inventory_orphan = inventory_context.map(|(orphan, _)| orphan).unwrap_or(false);
        let age_seconds = inventory_context
            .map(|(_, age_seconds)| age_seconds)
            .unwrap_or_default();
        Ok(json!({
            "exists": present,
            "running": exists && !stopped,
            "stopped": stopped,
            "lifecycle_state": lifecycle,
            "orphan": inventory_orphan,
            "age_seconds": age_seconds,
            "held_by": [],
        }))
    }

    async fn inventory_context(&self, action: &Action) -> Result<Option<(bool, u64)>, CmdError> {
        let Some(resource_id) = action.parameters.get("resource_id").and_then(Value::as_str) else {
            return Ok(None);
        };
        let Some(snapshot) = crate::autonomy::storage::read_json::<
            crate::autonomy::model::InventorySnapshot,
        >(&self.store, "state/autonomy/inventory/latest.json")
        .await?
        else {
            return Ok(Some((false, u64::default())));
        };
        let Some(resource) = snapshot
            .resources
            .iter()
            .find(|resource| resource.resource_id == resource_id)
        else {
            return Ok(Some((false, u64::default())));
        };
        let revision_matches = action
            .parameters
            .get("resource_revision")
            .and_then(Value::as_str)
            .is_some_and(|expected| resource.source_revision.as_deref() == Some(expected));
        let age_seconds = resource
            .created_at
            .as_deref()
            .and_then(|raw| chrono::DateTime::parse_from_rfc3339(raw).ok())
            .map(|created| {
                chrono::Utc::now()
                    .signed_duration_since(created.with_timezone(&chrono::Utc))
                    .num_seconds()
                    .max(i64::default())
            })
            .and_then(|seconds| u64::try_from(seconds).ok())
            .unwrap_or_default();
        Ok(Some((
            resource.ownership.is_mutable() && resource.workload.is_none() && revision_matches,
            age_seconds,
        )))
    }

    async fn delete_vm(&self, action: &Action) -> Result<Value, CmdError> {
        let before = self.inspect_vm(action).await?;
        if before.get("exists").and_then(Value::as_bool) == Some(false) {
            return Ok(json!({"already_absent": true}));
        }
        if before.get("orphan").and_then(Value::as_bool) != Some(true) {
            return Err(CmdError::click(format!(
                "refusing {}: ownership changed after planning",
                action.resource.reference
            )));
        }
        let provider = provider_name(action.resource.provider)?;
        get_provider(provider)?
            .delete_instance(&action.resource.reference)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        Ok(json!({"deleted": true, "provider": provider}))
    }

    async fn stop_vm(&self, action: &Action) -> Result<Value, CmdError> {
        let provider = provider_name(action.resource.provider)?;
        get_provider(provider)?
            .stop_instance(&action.resource.reference)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        Ok(json!({"stopped": true, "provider": provider}))
    }

    async fn start_vm(&self, action: &Action) -> Result<Value, CmdError> {
        let provider = provider_name(action.resource.provider)?;
        get_provider(provider)?
            .start_instance(&action.resource.reference)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        Ok(json!({"started": true, "provider": provider}))
    }

    fn gcp(&self) -> Result<&GcpRest, CmdError> {
        self.gcp
            .as_ref()
            .ok_or_else(|| CmdError::click("GCP executor was not initialized"))
    }
}

fn provider_name(provider: ProviderKind) -> Result<&'static str, CmdError> {
    crate::capabilities::constructible_variant(
        crate::capabilities::RuntimeFacet::Compute,
        provider.as_str(),
    )
    .map(|variant| variant.id)
    .ok_or_else(|| CmdError::click(format!("provider {provider:?} has no VM deletion executor")))
}
