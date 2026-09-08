//! Mutations: the apply and rollback half of each family, one typed method per
//! REST call, every one of them waiting for its operation to finish.

use reqwest::Method;
use serde_json::{json, Value};

use crate::cli::resources::model::{Action, Rollback};
use crate::cli::CmdError;

use super::paths::{
    address_path, disk_path, location, mig_path, parameter_str, reservation_path, scope,
};
use super::GcpRest;

impl GcpRest {
    pub(in crate::cli::resources::executors) async fn snapshot_disk(
        &self,
        action: &Action,
    ) -> Result<Value, CmdError> {
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

    pub(in crate::cli::resources::executors) async fn delete_disk(
        &self,
        action: &Action,
    ) -> Result<Value, CmdError> {
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

    pub(in crate::cli::resources::executors) async fn release_address(
        &self,
        action: &Action,
    ) -> Result<Value, CmdError> {
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

    pub(in crate::cli::resources::executors) async fn delete_mig(
        &self,
        action: &Action,
    ) -> Result<Value, CmdError> {
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

    pub(in crate::cli::resources::executors) async fn release_reservation(
        &self,
        action: &Action,
    ) -> Result<Value, CmdError> {
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

    pub(in crate::cli::resources::executors) async fn pause_scheduler(
        &self,
        action: &Action,
    ) -> Result<Value, CmdError> {
        let url = format!(
            "https://cloudscheduler.googleapis.com/v1/projects/{}/locations/{}/jobs/{}:pause",
            self.project,
            location(action)?,
            action.resource.name
        );
        self.request_json(Method::POST, &url, Some(&json!({})), "pause Scheduler job")
            .await
    }

    pub(in crate::cli::resources::executors) async fn resume_scheduler(
        &self,
        action: &Action,
    ) -> Result<Value, CmdError> {
        let url = format!(
            "https://cloudscheduler.googleapis.com/v1/projects/{}/locations/{}/jobs/{}:resume",
            self.project,
            location(action)?,
            action.resource.name
        );
        self.request_json(Method::POST, &url, Some(&json!({})), "resume Scheduler job")
            .await
    }

    pub(in crate::cli::resources::executors) async fn resize_mig(
        &self,
        action: &Action,
    ) -> Result<Value, CmdError> {
        self.resize_mig_with(action, &action.parameters).await
    }

    pub(in crate::cli::resources::executors) async fn resize_mig_with(
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

    pub(in crate::cli::resources::executors) async fn suspend_sql(
        &self,
        action: &Action,
    ) -> Result<Value, CmdError> {
        self.patch_sql(action, "NEVER").await
    }

    pub(in crate::cli::resources::executors) async fn restore_sql(
        &self,
        action: &Action,
        rollback: &Rollback,
    ) -> Result<Value, CmdError> {
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

    pub(in crate::cli::resources::executors) async fn restore_disk(
        &self,
        action: &Action,
        rollback: &Rollback,
    ) -> Result<Value, CmdError> {
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

    pub(in crate::cli::resources::executors) async fn delete_snapshot(
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
}
