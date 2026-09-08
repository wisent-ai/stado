//! Inspection: one read per resource family, reporting exactly the fields a
//! plan's preconditions and postconditions are written against.

use serde_json::{json, Value};

use crate::cli::resources::model::Action;
use crate::cli::CmdError;

use super::paths::{address_path, disk_path, json_u64, location, mig_path, reservation_path};
use super::GcpRest;

impl GcpRest {
    pub(in crate::cli::resources::executors) async fn inspect_disk(
        &self,
        action: &Action,
    ) -> Result<Value, CmdError> {
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

    pub(in crate::cli::resources::executors) async fn inspect_address(
        &self,
        action: &Action,
    ) -> Result<Value, CmdError> {
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

    pub(in crate::cli::resources::executors) async fn inspect_mig(
        &self,
        action: &Action,
    ) -> Result<Value, CmdError> {
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

    pub(in crate::cli::resources::executors) async fn inspect_reservation(
        &self,
        action: &Action,
    ) -> Result<Value, CmdError> {
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

    pub(in crate::cli::resources::executors) async fn inspect_scheduler(
        &self,
        action: &Action,
    ) -> Result<Value, CmdError> {
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

    pub(in crate::cli::resources::executors) async fn inspect_sql(
        &self,
        action: &Action,
    ) -> Result<Value, CmdError> {
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
}
