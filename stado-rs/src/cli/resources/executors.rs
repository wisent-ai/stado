//! Typed resource executors. Plans cannot inject arbitrary REST methods or URLs.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use reqwest::Method;
use serde_json::{json, Value};
use tempfile::NamedTempFile;

use crate::providers::get_provider;
use crate::queue::JobStorage;

use super::model::{Action, ActionKind, Condition, ProviderKind, Rollback};
use super::CmdError;

const CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

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
        >(&self.store, "autonomy/inventory/latest.json")
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

pub fn conditions_match(conditions: &[Condition], observed: &Value) -> bool {
    conditions.iter().all(|condition| {
        let actual = field(observed, &condition.field);
        match condition.field.as_str() {
            "minimum_age_seconds" => actual
                .and_then(Value::as_f64)
                .zip(condition.expected.as_f64())
                .is_some_and(|(actual, minimum)| actual >= minimum),
            _ => actual == Some(&condition.expected),
        }
    })
}

pub fn explain_mismatch(conditions: &[Condition], observed: &Value) -> String {
    conditions
        .iter()
        .filter_map(|condition| {
            let actual = field(observed, &condition.field);
            let matches = match condition.field.as_str() {
                "minimum_age_seconds" => actual
                    .and_then(Value::as_f64)
                    .zip(condition.expected.as_f64())
                    .is_some_and(|(actual, minimum)| actual >= minimum),
                _ => actual == Some(&condition.expected),
            };
            (!matches).then(|| {
                format!(
                    "{} expected {}, observed {}",
                    condition.field,
                    condition.expected,
                    actual.cloned().unwrap_or(Value::Null)
                )
            })
        })
        .collect::<Vec<String>>()
        .join("; ")
}

fn field<'a>(value: &'a Value, dotted: &str) -> Option<&'a Value> {
    dotted
        .split('.')
        .try_fold(value, |current, part| current.get(part))
}

fn provider_name(provider: ProviderKind) -> Result<&'static str, CmdError> {
    crate::capabilities::constructible_variant(
        crate::capabilities::RuntimeFacet::Compute,
        provider.as_str(),
    )
    .map(|variant| variant.id)
    .ok_or_else(|| CmdError::click(format!("provider {provider:?} has no VM deletion executor")))
}

#[derive(Clone)]
struct GcpRest {
    http: reqwest::Client,
    token: String,
    project: String,
}

impl GcpRest {
    async fn new(project: &str) -> Result<Self, CmdError> {
        let auth = crate::skarbiec::gcp_provider()
            .await
            .map_err(|error| CmdError::click(format!("GCP authentication failed: {error}")))?;
        let token = auth
            .token(&[CLOUD_PLATFORM_SCOPE])
            .await
            .map_err(|error| CmdError::click(format!("GCP token failed: {error}")))?;
        let http = reqwest::Client::builder()
            .user_agent(format!(
                "stado/{} resource-operations",
                env!("CARGO_PKG_VERSION")
            ))
            .timeout(Duration::from_secs(
                chrono::Duration::minutes(true as i64)
                    .num_seconds()
                    .try_into()
                    .unwrap_or_default(),
            ))
            .build()?;
        Ok(Self {
            http,
            token: token.as_str().to_string(),
            project: project.to_string(),
        })
    }

    async fn inspect_disk(&self, action: &Action) -> Result<Value, CmdError> {
        let disk_url = self.compute_url(&disk_path(action)?);
        let disk = self.get_allow_404(&disk_url, "inspect disk").await?;
        let snapshot_name = action
            .parameters
            .get("snapshot_name")
            .and_then(Value::as_str)
            .or_else(|| {
                action
                    .rollback
                    .as_ref()
                    .and_then(|rollback| rollback.parameters.get("snapshot_name"))
                    .and_then(Value::as_str)
            });
        let snapshot_exists = match snapshot_name {
            Some(name) => {
                let url = self.compute_url(&format!(
                    "/projects/{}/global/snapshots/{name}",
                    self.project
                ));
                self.get_allow_404(&url, "inspect recovery snapshot")
                    .await?
                    .is_some()
            }
            None => false,
        };
        Ok(match disk {
            None => json!({
                "exists": false,
                "unattached": true,
                "snapshot_exists": snapshot_exists,
            }),
            Some(disk) => json!({
                "exists": true,
                "unattached": disk.get("users").and_then(Value::as_array).is_none_or(Vec::is_empty),
                "snapshot_exists": snapshot_exists,
                "resource_id": disk.get("id"),
                "creation_timestamp": disk.get("creationTimestamp"),
                "fingerprint": disk.get("labelFingerprint"),
                "type_url": disk.get("type"),
                "type": disk
                    .get("type")
                    .and_then(Value::as_str)
                    .and_then(|value| value.rsplit('/').next()),
                "size_gb": disk.get("sizeGb"),
                "source_snapshot": disk
                    .get("sourceSnapshot")
                    .and_then(Value::as_str)
                    .and_then(|value| value.rsplit('/').next()),
                "labels": disk.get("labels"),
                "description": disk.get("description"),
                "replica_zones": disk.get("replicaZones"),
                "resource_policies": disk.get("resourcePolicies"),
                "physical_block_size_bytes": disk.get("physicalBlockSizeBytes"),
            }),
        })
    }

    async fn inspect_address(&self, action: &Action) -> Result<Value, CmdError> {
        let address = self
            .get_allow_404(
                &self.compute_url(&address_path(action)?),
                "inspect static address",
            )
            .await?;
        Ok(match address {
            None => json!({"exists": false, "unused": true}),
            Some(address) => json!({
                "exists": true,
                "unused": address.get("users").and_then(Value::as_array).is_none_or(Vec::is_empty),
                "resource_id": address.get("id"),
                "creation_timestamp": address.get("creationTimestamp"),
                "status": address.get("status"),
                "address": address.get("address"),
            }),
        })
    }

    async fn inspect_mig(&self, action: &Action) -> Result<Value, CmdError> {
        let group = self
            .get_allow_404(
                &self.compute_url(&mig_path(action)?),
                "inspect managed group",
            )
            .await?;
        Ok(match group {
            None => json!({"exists": false, "target_size": Value::Null}),
            Some(group) => json!({
                "exists": true,
                "target_size": group.get("targetSize"),
                "resource_id": group.get("id"),
                "creation_timestamp": group.get("creationTimestamp"),
                "fingerprint": group.get("fingerprint"),
            }),
        })
    }

    async fn inspect_reservation(&self, action: &Action) -> Result<Value, CmdError> {
        let value = self
            .get_allow_404(
                &self.compute_url(&reservation_path(action)?),
                "inspect reservation",
            )
            .await?;
        Ok(json!({
            "exists": value.is_some(),
            "status": value.as_ref().and_then(|item| item.get("status")),
            "resource_id": value.as_ref().and_then(|item| item.get("id")),
            "in_use_count": value
                .as_ref()
                .and_then(|item| item.pointer("/specificReservation/inUseCount"))
                .and_then(json_u64),
            "creation_timestamp": value.as_ref().and_then(|item| item.get("creationTimestamp")),
        }))
    }

    async fn inspect_scheduler(&self, action: &Action) -> Result<Value, CmdError> {
        let url = format!(
            "https://cloudscheduler.googleapis.com/v1/projects/{}/locations/{}/jobs/{}",
            self.project,
            location(action)?,
            action.resource.name
        );
        let value = self.get_allow_404(&url, "inspect Scheduler job").await?;
        Ok(match value {
            None => json!({"exists": false, "state": Value::Null}),
            Some(value) => json!({
                "exists": true,
                "state": value.get("state"),
                "etag": value.get("etag"),
            }),
        })
    }

    async fn inspect_sql(&self, action: &Action) -> Result<Value, CmdError> {
        let url = format!(
            "https://sqladmin.googleapis.com/sql/v1beta4/projects/{}/instances/{}",
            self.project, action.resource.name
        );
        let value = self.get_allow_404(&url, "inspect Cloud SQL").await?;
        Ok(match value {
            None => json!({"exists": false}),
            Some(value) => json!({
                "exists": true,
                "activation_policy": value.pointer("/settings/activationPolicy"),
                "state": value.get("state"),
                "settings_version": value.pointer("/settings/settingsVersion"),
                "etag": value.get("etag"),
            }),
        })
    }

    async fn snapshot_disk(&self, action: &Action) -> Result<Value, CmdError> {
        let snapshot_name = parameter_str(&action.parameters, "snapshot_name", action)?;
        let path = format!("{}/createSnapshot", disk_path(action)?);
        let operation = self
            .request_json(
                Method::POST,
                &self.compute_url(&path),
                Some(&json!({"name": snapshot_name})),
                "snapshot disk",
            )
            .await?;
        self.wait_operation(&operation).await?;
        Ok(json!({"snapshot_name": snapshot_name}))
    }

    async fn delete_disk(&self, action: &Action) -> Result<Value, CmdError> {
        let operation = self
            .request_json(
                Method::DELETE,
                &self.compute_url(&disk_path(action)?),
                None,
                "delete disk",
            )
            .await?;
        self.wait_operation(&operation).await?;
        Ok(json!({"deleted": true}))
    }

    async fn release_address(&self, action: &Action) -> Result<Value, CmdError> {
        let operation = self
            .request_json(
                Method::DELETE,
                &self.compute_url(&address_path(action)?),
                None,
                "release address",
            )
            .await?;
        self.wait_operation(&operation).await?;
        Ok(json!({"released": true}))
    }

    async fn delete_mig(&self, action: &Action) -> Result<Value, CmdError> {
        let operation = self
            .request_json(
                Method::DELETE,
                &self.compute_url(&mig_path(action)?),
                None,
                "delete managed instance group",
            )
            .await?;
        self.wait_operation(&operation).await?;
        Ok(json!({"deleted": true}))
    }

    async fn release_reservation(&self, action: &Action) -> Result<Value, CmdError> {
        let operation = self
            .request_json(
                Method::DELETE,
                &self.compute_url(&reservation_path(action)?),
                None,
                "release reservation",
            )
            .await?;
        self.wait_operation(&operation).await?;
        Ok(json!({"released": true}))
    }

    async fn pause_scheduler(&self, action: &Action) -> Result<Value, CmdError> {
        let url = format!(
            "https://cloudscheduler.googleapis.com/v1/projects/{}/locations/{}/jobs/{}:pause",
            self.project,
            location(action)?,
            action.resource.name
        );
        self.request_json(Method::POST, &url, Some(&json!({})), "pause Scheduler job")
            .await
    }

    async fn resume_scheduler(&self, action: &Action) -> Result<Value, CmdError> {
        let url = format!(
            "https://cloudscheduler.googleapis.com/v1/projects/{}/locations/{}/jobs/{}:resume",
            self.project,
            location(action)?,
            action.resource.name
        );
        self.request_json(Method::POST, &url, Some(&json!({})), "resume Scheduler job")
            .await
    }

    async fn resize_mig(&self, action: &Action) -> Result<Value, CmdError> {
        self.resize_mig_with(action, &action.parameters).await
    }

    async fn resize_mig_with(
        &self,
        action: &Action,
        parameters: &Value,
    ) -> Result<Value, CmdError> {
        let target = parameters
            .get("target_size")
            .and_then(Value::as_i64)
            .ok_or_else(|| CmdError::click(format!("action {} has no target_size", action.id)))?;
        let url = self.compute_url(&format!("{}/resize?size={target}", mig_path(action)?));
        let operation = self
            .request_json(Method::POST, &url, Some(&json!({})), "resize managed group")
            .await?;
        self.wait_operation(&operation).await?;
        Ok(json!({"target_size": target}))
    }

    async fn suspend_sql(&self, action: &Action) -> Result<Value, CmdError> {
        self.patch_sql(action, "NEVER").await
    }

    async fn restore_sql(&self, action: &Action, rollback: &Rollback) -> Result<Value, CmdError> {
        let policy = parameter_str(&rollback.parameters, "activation_policy", action)?;
        self.patch_sql(action, policy).await
    }

    async fn patch_sql(&self, action: &Action, policy: &str) -> Result<Value, CmdError> {
        let url = format!(
            "https://sqladmin.googleapis.com/sql/v1beta4/projects/{}/instances/{}",
            self.project, action.resource.name
        );
        let current = self.inspect_sql(action).await?;
        let mut settings = json!({"activationPolicy": policy});
        if let Some(version) = current
            .get("settings_version")
            .filter(|value| !value.is_null())
        {
            settings["settingsVersion"] = version.clone();
        }
        let body = json!({"settings": settings});
        let operation = self
            .request_json(
                Method::PATCH,
                &url,
                Some(&body),
                "change Cloud SQL activation policy",
            )
            .await?;
        self.wait_operation(&operation).await?;
        Ok(json!({"activation_policy": policy}))
    }

    async fn restore_disk(&self, action: &Action, rollback: &Rollback) -> Result<Value, CmdError> {
        let snapshot_name = parameter_str(&rollback.parameters, "snapshot_name", action)?;
        let snapshot = self.compute_url(&format!(
            "/projects/{}/global/snapshots/{snapshot_name}",
            self.project
        ));
        let regional = scope(action) == "region";
        let path = if regional {
            format!(
                "/projects/{}/regions/{}/disks",
                self.project,
                location(action)?
            )
        } else {
            format!(
                "/projects/{}/zones/{}/disks",
                self.project,
                location(action)?
            )
        };
        let original = rollback
            .parameters
            .get("original")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                CmdError::click(format!(
                    "action {} has no original disk metadata",
                    action.id
                ))
            })?;
        let mut body = json!({"name": action.resource.name, "sourceSnapshot": snapshot});
        for (source, target) in [
            ("size_gb", "sizeGb"),
            ("labels", "labels"),
            ("description", "description"),
            ("replica_zones", "replicaZones"),
            ("resource_policies", "resourcePolicies"),
            ("physical_block_size_bytes", "physicalBlockSizeBytes"),
        ] {
            if let Some(value) = original.get(source).filter(|value| !value.is_null()) {
                body[target] = value.clone();
            }
        }
        let disk_type = original
            .get("type_url")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CmdError::click(format!(
                    "action {} has no original disk type URL",
                    action.id
                ))
            })?;
        body["type"] = Value::String(disk_type.to_string());
        let operation = self
            .request_json(
                Method::POST,
                &self.compute_url(&path),
                Some(&body),
                "restore disk from snapshot",
            )
            .await?;
        self.wait_operation(&operation).await?;
        Ok(json!({"restored_from": snapshot_name}))
    }

    async fn delete_snapshot(
        &self,
        action: &Action,
        rollback: &Rollback,
    ) -> Result<Value, CmdError> {
        let snapshot_name = parameter_str(&rollback.parameters, "snapshot_name", action)?;
        let url = self.compute_url(&format!(
            "/projects/{}/global/snapshots/{snapshot_name}",
            self.project
        ));
        let operation = self
            .request_json(Method::DELETE, &url, None, "delete recovery snapshot")
            .await?;
        self.wait_operation(&operation).await?;
        Ok(json!({"deleted_snapshot": snapshot_name}))
    }

    async fn get_allow_404(&self, url: &str, description: &str) -> Result<Option<Value>, CmdError> {
        let response = self.http.get(url).bearer_auth(&self.token).send().await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(api_error(description, status, &text));
        }
        serde_json::from_str(&text).map(Some).map_err(|error| {
            CmdError::click(format!("{description} returned invalid JSON: {error}"))
        })
    }

    async fn request_json(
        &self,
        method: Method,
        url: &str,
        body: Option<&Value>,
        description: &str,
    ) -> Result<Value, CmdError> {
        let mut request = self.http.request(method, url).bearer_auth(&self.token);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(json!({"already_absent": true}));
        }
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(api_error(description, status, &text));
        }
        if text.trim().is_empty() {
            Ok(Value::Null)
        } else {
            serde_json::from_str(&text).map_err(|error| {
                CmdError::click(format!("{description} returned invalid JSON: {error}"))
            })
        }
    }

    async fn wait_operation(&self, operation: &Value) -> Result<(), CmdError> {
        if operation.get("already_absent").and_then(Value::as_bool) == Some(true)
            || operation.is_null()
        {
            return Ok(());
        }
        let url = operation
            .get("selfLink")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                (operation.get("kind").and_then(Value::as_str) == Some("sql#operation"))
                    .then(|| {
                        operation.get("name").and_then(Value::as_str).map(|name| {
                            format!(
                                "https://sqladmin.googleapis.com/sql/v1beta4/projects/{}/operations/{name}",
                                self.project
                            )
                        })
                    })
                    .flatten()
            });
        let Some(url) = url else {
            return Ok(());
        };
        let deadline = Instant::now()
            + Duration::from_secs(
                chrono::Duration::hours(true as i64)
                    .num_seconds()
                    .try_into()
                    .unwrap_or_default(),
            );
        loop {
            let value = self
                .get_allow_404(&url, "poll resource operation")
                .await?
                .ok_or_else(|| CmdError::click("resource operation disappeared while polling"))?;
            if value.get("status").and_then(Value::as_str) == Some("DONE") {
                if value.get("error").is_some_and(|error| !error.is_null()) {
                    return Err(CmdError::click(format!(
                        "resource operation failed: {}",
                        value["error"]
                    )));
                }
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(CmdError::click(format!(
                    "resource operation did not finish before timeout: {url}"
                )));
            }
            tokio::time::sleep(Duration::from_secs(true as u64)).await;
        }
    }

    fn compute_url(&self, path: &str) -> String {
        format!("https://compute.googleapis.com/compute/v1{path}")
    }
}

fn disk_path(action: &Action) -> Result<String, CmdError> {
    Ok(match scope(action) {
        "region" => format!(
            "/projects/{}/regions/{}/disks/{}",
            project(action)?,
            location(action)?,
            action.resource.name
        ),
        _ => format!(
            "/projects/{}/zones/{}/disks/{}",
            project(action)?,
            location(action)?,
            action.resource.name
        ),
    })
}

fn address_path(action: &Action) -> Result<String, CmdError> {
    Ok(if scope(action) == "global" {
        format!(
            "/projects/{}/global/addresses/{}",
            project(action)?,
            action.resource.name
        )
    } else {
        format!(
            "/projects/{}/regions/{}/addresses/{}",
            project(action)?,
            location(action)?,
            action.resource.name
        )
    })
}

fn mig_path(action: &Action) -> Result<String, CmdError> {
    Ok(match scope(action) {
        "region" => format!(
            "/projects/{}/regions/{}/instanceGroupManagers/{}",
            project(action)?,
            location(action)?,
            action.resource.name
        ),
        _ => format!(
            "/projects/{}/zones/{}/instanceGroupManagers/{}",
            project(action)?,
            location(action)?,
            action.resource.name
        ),
    })
}

fn reservation_path(action: &Action) -> Result<String, CmdError> {
    Ok(format!(
        "/projects/{}/zones/{}/reservations/{}",
        project(action)?,
        location(action)?,
        action.resource.name
    ))
}

fn scope(action: &Action) -> &str {
    action
        .parameters
        .get("scope")
        .and_then(Value::as_str)
        .unwrap_or("zone")
}

fn project(action: &Action) -> Result<&str, CmdError> {
    action
        .resource
        .project
        .as_deref()
        .ok_or_else(|| CmdError::click(format!("action {} has no project", action.id)))
}

fn location(action: &Action) -> Result<&str, CmdError> {
    action
        .resource
        .location
        .as_deref()
        .ok_or_else(|| CmdError::click(format!("action {} has no location", action.id)))
}

fn parameter_str<'a>(value: &'a Value, key: &str, action: &Action) -> Result<&'a str, CmdError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| CmdError::click(format!("action {} has no {key}", action.id)))
}

fn json_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
}

fn inspect_backup_config() -> Result<Value, CmdError> {
    if backup_env_override() {
        return Ok(json!({
            "configured": true,
            "mutable": false,
            "reason": "backup configuration is overridden by environment variables",
        }));
    }
    let path = crate::config_file::config_path()?
        .ok_or_else(|| CmdError::click("no writable Stado config file is active"))?;
    let root: Value = serde_json::from_slice(&fs::read(&path)?)?;
    let backup = root.pointer("/storage/backup").cloned();
    Ok(json!({
        "configured": backup.as_ref().is_some_and(|value| !value.is_null()),
        "mutable": true,
        "path": path,
        "backup": backup,
    }))
}

fn disable_backup_config(action: &Action) -> Result<Value, CmdError> {
    let state = inspect_backup_config()?;
    if state.get("mutable").and_then(Value::as_bool) != Some(true) {
        return Err(CmdError::click(
            state
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("backup configuration is not mutable"),
        ));
    }
    if !conditions_match(&action.preconditions, &state) {
        return Err(CmdError::click(format!(
            "backup configuration drifted before action {}",
            action.id
        )));
    }
    let path = state
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| CmdError::click("backup config inspection returned no path"))?;
    let mut root: Value = serde_json::from_slice(&fs::read(path)?)?;
    if root.pointer("/storage/backup") != state.get("backup") {
        return Err(CmdError::click(format!(
            "backup configuration drifted while applying action {}",
            action.id
        )));
    }
    let storage = root
        .get_mut("storage")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| CmdError::click("Stado config has no storage object"))?;
    let previous = storage.remove("backup").unwrap_or(Value::Null);
    atomic_json(Path::new(path), &root)?;
    Ok(json!({"previous_backup": previous, "config_path": path}))
}

fn enable_backup_config(action: &Action, receipt: Option<&Value>) -> Result<Value, CmdError> {
    let path = receipt
        .and_then(|value| value.get("config_path"))
        .and_then(Value::as_str)
        .map(|value| Path::new(value).to_path_buf())
        .or(crate::config_file::config_path()?)
        .ok_or_else(|| CmdError::click("backup restore has no writable config path"))?;
    let backup = receipt
        .and_then(|value| value.get("previous_backup"))
        .or_else(|| action.parameters.get("previous"))
        .cloned()
        .filter(|value| !value.is_null())
        .ok_or_else(|| CmdError::click("backup restore has no previous value"))?;
    let mut root: Value = serde_json::from_slice(&fs::read(&path)?)?;
    if root
        .pointer("/storage/backup")
        .is_some_and(|value| !value.is_null())
    {
        return Err(CmdError::click(
            "backup configuration was replaced before restore",
        ));
    }
    let storage = root
        .get_mut("storage")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| CmdError::click("Stado config has no storage object"))?;
    storage.insert("backup".to_string(), backup);
    atomic_json(&path, &root)?;
    Ok(json!({"restored": true, "config_path": path}))
}

fn backup_env_override() -> bool {
    crate::capabilities::STORAGE_BACKEND_CONFIG
        .backup_env
        .into_iter()
        .chain(crate::capabilities::backup_config_envs(
            crate::capabilities::RuntimeFacet::Storage,
        ))
        .any(|name| std::env::var(name).is_ok_and(|value| !value.trim().is_empty()))
}

fn atomic_json(path: &Path, value: &Value) -> Result<(), CmdError> {
    let parent = path
        .parent()
        .ok_or_else(|| CmdError::click(format!("{} has no parent", path.display())))?;
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    let mut temporary = NamedTempFile::new_in(parent)?;
    temporary.write_all(&bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

fn api_error(description: &str, status: reqwest::StatusCode, body: &str) -> CmdError {
    CmdError::click(format!(
        "{description} -> HTTP {}: {}",
        status.as_u16(),
        body.chars().take(u8::MAX as usize).collect::<String>()
    ))
}
