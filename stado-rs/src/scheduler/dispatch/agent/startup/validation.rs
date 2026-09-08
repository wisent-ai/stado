//! The source-level export contract a live template must satisfy, and the
//! storage settings that must be present before substitution can hide an
//! omitted one.

use std::collections::BTreeMap;

use crate::scheduler::scheduler::SchedulerError;

/// Read by the export check in `super::render`.
pub(super) const REQUIRED_AGENT_EXPORTS: &[&str] = &[
    "WC_STORAGE_BACKEND",
    "WC_BUCKET",
    "WC_AZURE_STORAGE_ACCOUNT",
    "WC_AZURE_CONTAINER",
    "WC_S3_BUCKET",
    "WC_S3_REGION",
    "WC_LOCAL_STORAGE_PATH",
    "WC_STADO_STORAGE_URL",
    "WC_STADO_STORAGE_TOKEN_FILE",
    "WC_STADO_STORAGE_NAMESPACE",
    "WC_BACKUP_STORAGE_BACKEND",
    "WC_BACKUP_BUCKET",
    "WC_BACKUP_AZURE_STORAGE_ACCOUNT",
    "WC_BACKUP_AZURE_CONTAINER",
    "WC_BACKUP_S3_REGION",
    "WC_BACKUP_LOCAL_STORAGE_PATH",
    "WC_AGENT_SKARBIEC_URL",
    "WC_AGENT_SKARBIEC_CONSUMER",
    "WC_AGENT_SKARBIEC_ITEMS",
    "WC_AGENT_SKARBIEC_SECRET_FIELDS",
];

/// Also called directly by `super::render` for the non-storage keys.
pub(super) fn require_deployment_setting(
    deployment: &BTreeMap<String, String>,
    key: &'static str,
    config_key: &'static str,
) -> Result<(), SchedulerError> {
    if deployment
        .get(key)
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Ok(());
    }
    Err(SchedulerError::MissingStartupSetting {
        key: key.to_string(),
        env: key,
        config_key,
    })
}

/// Called by `super::render` once the export contract holds.
pub(super) fn validate_storage_settings(
    deployment: &BTreeMap<String, String>,
) -> Result<(), SchedulerError> {
    require_deployment_setting(deployment, "WC_STORAGE_BACKEND", "storage.backend")?;
    match deployment.get("WC_STORAGE_BACKEND").map(String::as_str) {
        Some("gcs") => require_deployment_setting(deployment, "WC_BUCKET", "storage.gcs.bucket")?,
        Some("azure") => {
            require_deployment_setting(
                deployment,
                "WC_AZURE_STORAGE_ACCOUNT",
                "storage.azure.account",
            )?;
            require_deployment_setting(
                deployment,
                "WC_AZURE_CONTAINER",
                "storage.azure.container",
            )?;
        }
        Some("s3") => {
            require_deployment_setting(deployment, "WC_S3_BUCKET", "storage.s3.bucket")?;
        }
        Some("local") => {
            require_deployment_setting(deployment, "WC_LOCAL_STORAGE_PATH", "storage.local.path")?;
        }
        Some(_) | None => {
            return Err(SchedulerError::InvalidStartupSetting {
                key: "WC_STORAGE_BACKEND".to_string(),
                env: "WC_STORAGE_BACKEND",
                config_key: "storage.backend",
                reason: "expected gcs, azure, s3, or local",
            });
        }
    }
    match deployment
        .get("WC_BACKUP_STORAGE_BACKEND")
        .map(String::as_str)
    {
        Some("gcs") => {
            require_deployment_setting(deployment, "WC_BACKUP_BUCKET", "storage.backup.bucket")?
        }
        Some("azure") => {
            require_deployment_setting(
                deployment,
                "WC_BACKUP_AZURE_STORAGE_ACCOUNT",
                "storage.backup.azure.account",
            )?;
            require_deployment_setting(
                deployment,
                "WC_BACKUP_AZURE_CONTAINER",
                "storage.backup.azure.container",
            )?;
        }
        Some("s3") => {
            require_deployment_setting(deployment, "WC_BACKUP_BUCKET", "storage.backup.bucket")?
        }
        Some("local") => require_deployment_setting(
            deployment,
            "WC_BACKUP_LOCAL_STORAGE_PATH",
            "storage.backup.local.path",
        )?,
        Some("") | None => {}
        Some(_) => {
            return Err(SchedulerError::InvalidStartupSetting {
                key: "WC_BACKUP_STORAGE_BACKEND".to_string(),
                env: "WC_BACKUP_STORAGE_BACKEND",
                config_key: "storage.backup.backend",
                reason: "expected empty, gcs, azure, s3, or local",
            });
        }
    }
    Ok(())
}
