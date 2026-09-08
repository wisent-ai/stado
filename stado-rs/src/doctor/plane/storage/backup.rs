//! The replica the deployment is recoverable from, and the identity that
//! reaches it.

use crate::config;
use crate::coordinator;
use crate::doctor::{provider_enabled, storage_adapter, Check};
use crate::queue::copy::Endpoint;

pub(in crate::doctor) const BACKUP_ID: &str = "backup";
pub(in crate::doctor) const BACKUP_TITLE: &str = "Disaster-recovery replica";
pub(in crate::doctor) const BACKUP_REMEDY: &str =
    "set storage.backup backend/bucket/region and provision the provider adapter's \
     workload identity; backup credentials must not be exposed through agent.skarbiec.items \
     or agent.skarbiec.secret_fields";

pub(in crate::doctor) async fn check_backup(store_error: &str) -> Check {
    let primary_adapter = storage_adapter(config::wc_storage_backend());
    let backup_adapter = storage_adapter(config::wc_backup_storage_backend());
    let azure_cutover = primary_adapter == Some(crate::capabilities::StorageAdapter::AzureBlob)
        || provider_enabled(crate::capabilities::ProviderId::Azure);
    if !azure_cutover && primary_adapter == Some(crate::capabilities::StorageAdapter::Local) {
        if backup_adapter != Some(crate::capabilities::StorageAdapter::Local)
            || config::wc_backup_local_storage_path().is_empty()
            || config::wc_backup_local_storage_path() == config::wc_local_storage_path()
        {
            return Check::fail(
                BACKUP_ID,
                BACKUP_TITLE,
                "local outage profile needs a distinct storage.backup.local.path; use \
                 ~/.stado/local-backup"
                    .to_string(),
                "configure a distinct local backup path; it is temporary same-disk protection, \
                 not cross-provider disaster recovery",
            );
        }
        let Some(endpoint) = Endpoint::configured_backup() else {
            return Check::fail(
                BACKUP_ID,
                BACKUP_TITLE,
                "local backup endpoint did not resolve".to_string(),
                "set storage.backup.backend=local and storage.backup.local.path",
            );
        };
        let backend = match endpoint.build().await {
            Ok(backend) => backend,
            Err(error) => {
                return Check::fail(
                    BACKUP_ID,
                    BACKUP_TITLE,
                    format!(
                        "local backup cannot be opened at {}: {error}",
                        endpoint.describe()
                    ),
                    "create an owner-writable ~/.stado/local-backup directory",
                )
            }
        };
        if let Err(error) = backend
            .list_blobs_with_meta("diagnostics/backup-access/")
            .await
        {
            return Check::fail(
                BACKUP_ID,
                BACKUP_TITLE,
                format!(
                    "local backup cannot be listed at {}: {error}",
                    endpoint.describe()
                ),
                "create an owner-writable ~/.stado/local-backup directory",
            );
        }
        if !store_error.is_empty() {
            return Check::fail(
                BACKUP_ID,
                BACKUP_TITLE,
                format!("local primary plus backup could not be constructed: {store_error}"),
                "create owner-writable ~/.stado/local-storage and ~/.stado/local-backup directories",
            );
        }
        return Check::pass(
            BACKUP_ID,
            BACKUP_TITLE,
            format!(
                "{} mirrors the local primary with read fallback; this is same-disk temporary \
                 protection only",
                endpoint.describe()
            ),
            "restore required Azure-primary plus S3 cross-provider disaster recovery after the \
             tenant block is removed",
        );
    }
    if !azure_cutover {
        return Check::pass(
            BACKUP_ID,
            BACKUP_TITLE,
            "Azure cutover is not active; no mandatory S3 replica".to_string(),
            BACKUP_REMEDY,
        );
    }
    if backup_adapter != Some(crate::capabilities::StorageAdapter::S3) {
        return Check::fail(
            BACKUP_ID,
            BACKUP_TITLE,
            format!(
                "Azure cutover requires WC_BACKUP_STORAGE_BACKEND=s3, got {:?}; no automatic \
                 writer promotion is permitted",
                config::wc_backup_storage_backend()
            ),
            BACKUP_REMEDY,
        );
    }
    if config::wc_backup_bucket().is_empty() || config::wc_backup_s3_region().is_empty() {
        return Check::fail(
            BACKUP_ID,
            BACKUP_TITLE,
            format!(
                "S3 replica locator unresolved: bucket={:?} region={:?}",
                config::wc_backup_bucket(),
                config::wc_backup_s3_region()
            ),
            BACKUP_REMEDY,
        );
    }
    let Some(endpoint) = Endpoint::configured_backup() else {
        return Check::fail(
            BACKUP_ID,
            BACKUP_TITLE,
            "S3 replica endpoint did not resolve from the configured backup locator".to_string(),
            BACKUP_REMEDY,
        );
    };
    let backend = match endpoint.build().await {
        Ok(backend) => backend,
        Err(error) => {
            return Check::fail(
                BACKUP_ID,
                BACKUP_TITLE,
                format!("S3 replica provider identity could not be resolved: {error}"),
                BACKUP_REMEDY,
            )
        }
    };
    if let Err(error) = backend
        .list_blobs_with_meta("diagnostics/backup-access/")
        .await
    {
        return Check::fail(
            BACKUP_ID,
            BACKUP_TITLE,
            format!(
                "S3 replica provider identity lacks list access at {}: {error}",
                endpoint.describe()
            ),
            BACKUP_REMEDY,
        );
    }
    if !store_error.is_empty() {
        return Check::fail(
            BACKUP_ID,
            BACKUP_TITLE,
            format!(
                "Azure primary plus S3 backup could not be constructed; backup provider \
                 identity, bucket, or primary managed identity is unresolved: {store_error}"
            ),
            BACKUP_REMEDY,
        );
    }
    match coordinator::agent_workload_grant().await {
        Ok(Some(_)) => Check::pass(
            BACKUP_ID,
            BACKUP_TITLE,
            format!(
                "provider adapter can list S3 replica s3://{} in {}; dedicated agent consumer \
                 {:?} exposes exactly its provider-neutral workload items",
                config::wc_backup_bucket(),
                config::wc_backup_s3_region(),
                config::agent_skarbiec_consumer()
            ),
            BACKUP_REMEDY,
        ),
        Ok(None) => Check::fail(
            BACKUP_ID,
            BACKUP_TITLE,
            "Azure workload grant was not resolved; dispatch is fenced".to_string(),
            BACKUP_REMEDY,
        ),
        Err(error) => Check::fail(
            BACKUP_ID,
            BACKUP_TITLE,
            format!("Azure workload grant is absent, unreachable, or overbroad: {error}"),
            BACKUP_REMEDY,
        ),
    }
}
