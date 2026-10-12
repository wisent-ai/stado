//! Findings that need no cloud call: the declared storage topology, the
//! compute providers the fence keeps out of scheduling, and the agent VMs the
//! authoritative queue and lease store cannot account for.

use std::fs;

use serde_json::{json, Value};

use crate::cli::instances;
use crate::cli::resources::rationalize::{Finding, Level, Recommendation, ResourceType};
use crate::queue::copy::Endpoint;

use super::fields::finding;

/// The configured `storage.backup`, as the findings' evidence. Null when no
/// config file exists or it declares none; a config that cannot be located,
/// read or parsed answers `{"unreadable": cause}` so the evidence never reads
/// as "nothing configured" when nothing was read.
fn configured_backup_value() -> Value {
    let path = match crate::config_file::config_path() {
        Ok(Some(path)) => path,
        Ok(None) => return Value::Null,
        Err(error) => return json!({ "unreadable": format!("the config path: {error}") }),
    };
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => return json!({ "unreadable": format!("{}: {error}", path.display()) }),
    };
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(root) => match root.pointer("/storage/backup") {
            Some(backup) => backup.clone(),
            None => Value::Null,
        },
        Err(error) => json!({ "unreadable": format!("{} is not JSON: {error}", path.display()) }),
    }
}

pub(super) fn configuration_findings(
    primary: &Endpoint,
    backup: Option<&Endpoint>,
    active: &[String],
    disabled: &[String],
) -> Vec<Finding> {
    let mut findings = Vec::new();
    let primary_local = primary.adapter() == Some(crate::capabilities::StorageAdapter::Local);
    let backup_config = configured_backup_value();
    if let Some(backup) = backup {
        if primary.describe() == backup.describe() {
            findings.push(finding(
                "storage-duplicate-backup",
                Level::High,
                Recommendation::Disable,
                Level::High,
                "stado",
                ResourceType::StorageBackup,
                backup.describe(),
                "primary and backup resolve to the same store, so the backup consumes work without adding a failure domain",
                json!({
                    "primary": primary.describe(),
                    "backup": backup.describe(),
                    "backup_config": backup_config.clone(),
                }),
            ));
        } else if primary_local
            && backup.adapter() == Some(crate::capabilities::StorageAdapter::Local)
        {
            findings.push(finding(
                "storage-local-only-backup",
                Level::Medium,
                Recommendation::Move,
                Level::High,
                crate::capabilities::ProviderId::Local.as_str(),
                ResourceType::StorageBackup,
                backup.describe(),
                "a local primary and local backup remain in the same device failure domain; move the backup off-host or disable the misleading replica",
                json!({
                    "primary": primary.describe(),
                    "backup": backup.describe(),
                    "backup_config": backup_config,
                }),
            ));
        }
    }

    let active_remote: Vec<&str> = active
        .iter()
        .map(String::as_str)
        .filter(|provider| !crate::capabilities::ProviderId::Local.matches(provider))
        .collect();
    if primary_local && !active_remote.is_empty() {
        findings.push(finding(
            "local-storage-with-remote-compute",
            Level::High,
            Recommendation::DisableOrMigrate,
            Level::High,
            "stado",
            ResourceType::Configuration,
            primary.describe(),
            "remote agents cannot share a device-local queue reliably; migrate storage to a shared backend or keep remote providers disabled",
            json!({"active_remote_providers": active_remote}),
        ));
    }

    for provider in disabled {
        findings.push(finding(
            &format!("disabled-provider-{provider}"),
            Level::Low,
            Recommendation::ReviewDeprovision,
            Level::Medium,
            provider,
            ResourceType::ProviderFence,
            format!("provider:{provider}"),
            "the provider is fenced from scheduling; if that fence is permanent, remove its Stado-owned network, VM, reservation and credential resources",
            json!({"configured_state": "disabled"}),
        ));
    }
    findings
}

pub(super) fn orphan_instance_findings(
    rows: &[instances::AuditInstanceRow],
    min_age_seconds: u64,
) -> Vec<Finding> {
    rows.iter()
        .filter(|row| row.is_orphan() && row.age_seconds >= min_age_seconds as f64)
        .map(|row| {
            let mut candidate = finding(
                &format!("orphan-vm-{}-{}", row.provider, row.reference),
                Level::High,
                Recommendation::Delete,
                Level::High,
                &row.provider,
                ResourceType::AgentVm,
                row.reference.clone(),
                "the live agent VM has no running job and no unexpired provider lease in the authoritative store",
                json!({
                    "age_seconds": row.age_seconds,
                    "accelerator": row.accel,
                    "held_by": row.held_by,
                }),
            );
            candidate.automatic = true;
            candidate
        })
        .collect()
}
