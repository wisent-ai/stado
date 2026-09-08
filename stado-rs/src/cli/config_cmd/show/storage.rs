//! The `wc_*` stretch of `config show`: which providers run work, which
//! backend holds the working data, and which backend holds its backups.

use serde_json::{Map, Value};

use crate::config;

pub(super) fn insert(resolved: &mut Map<String, Value>) {
    resolved.insert(
        "wc_providers".into(),
        Value::Array(
            config::wc_providers()
                .iter()
                .map(|p| Value::from(p.as_str()))
                .collect(),
        ),
    );
    resolved.insert(
        "wc_disabled_providers".into(),
        Value::Array(
            config::wc_disabled_providers()
                .iter()
                .map(|p| Value::from(p.as_str()))
                .collect(),
        ),
    );
    resolved.insert(
        "wc_storage_backend".into(),
        Value::from(config::wc_storage_backend()),
    );
    resolved.insert(
        "wc_local_storage_path".into(),
        Value::from(config::wc_local_storage_path()),
    );
    resolved.insert(
        "wc_backup_storage_backend".into(),
        Value::from(config::wc_backup_storage_backend()),
    );
    resolved.insert(
        "wc_backup_bucket".into(),
        Value::from(config::wc_backup_bucket()),
    );
    resolved.insert(
        "wc_backup_azure_storage_account".into(),
        Value::from(config::wc_backup_azure_storage_account()),
    );
    resolved.insert(
        "wc_backup_azure_container".into(),
        Value::from(config::wc_backup_azure_container()),
    );
    resolved.insert(
        "wc_backup_s3_region".into(),
        Value::from(config::wc_backup_s3_region()),
    );
    resolved.insert(
        "wc_backup_local_storage_path".into(),
        Value::from(config::wc_backup_local_storage_path()),
    );
}
