//! The document's own contract and the two storage backends it names: the
//! schema version this binary supports, the credential store and alert
//! channels, and the primary queue store with the replica the catalog may
//! require alongside it.

use serde_json::{Map, Value};

use super::helpers::{catalog_variant, validate_variant_config};
use crate::config_file::readers::{binding_in, field_in};
use crate::config_file::SCHEMA_VERSION;

/// The one version of the document contract this binary can read.
pub(super) fn schema_version(root: &Map<String, Value>, problems: &mut Vec<String>) {
    match root.get("schema_version").and_then(Value::as_u64) {
        Some(version) if version == u64::from(SCHEMA_VERSION) => {}
        Some(version) => problems.push(format!(
            "unsupported config schema_version {version}; expected {SCHEMA_VERSION}"
        )),
        None => problems.push(format!(
            "config schema_version is required; expected {SCHEMA_VERSION}"
        )),
    }
}

/// The credential store selector, its admin grant, and the alert channels —
/// each judged against the catalog rather than against a literal list.
pub(super) fn credentials_and_alerts(root: &Map<String, Value>, problems: &mut Vec<String>) {
    if let Some(store) = field_in(root, &crate::capabilities::CREDENTIALS_STORE_CONFIG) {
        match store.as_str().filter(|value| !value.trim().is_empty()) {
            Some(store) => {
                if let Err(error) = crate::credential_store::parse_selector(store) {
                    problems.push(error.to_string());
                }
            }
            None => problems.push("credentials.store must be a non-empty string".to_string()),
        }
    }
    for field in [
        &crate::capabilities::CREDENTIALS_ADMIN_CONSUMER_CONFIG,
        &crate::capabilities::CREDENTIALS_ADMIN_TOKEN_FILE_CONFIG,
    ] {
        if field_in(root, field)
            .is_some_and(|value| !value.as_str().is_some_and(|entry| !entry.trim().is_empty()))
        {
            problems.push(format!("{} must be a non-empty string", field.path));
        }
    }
    if let Some(channels) = field_in(root, &crate::capabilities::ALERT_CHANNELS_CONFIG) {
        match channels {
            Value::Array(values) => {
                let supported = crate::capabilities::configurable_ids(
                    crate::capabilities::RuntimeFacet::Alerts,
                )
                .collect::<std::collections::BTreeSet<_>>();
                for value in values {
                    match value.as_str() {
                        Some(channel) if supported.contains(channel) => {}
                        Some(channel) => problems.push(format!(
                            "alerts.channels contains unsupported channel {channel:?}"
                        )),
                        None => {
                            problems.push("alerts.channels entries must be strings".to_string())
                        }
                    }
                }
            }
            _ => problems.push("alerts.channels must be an array".to_string()),
        }
    }
}

/// The primary queue store, the replica an operator may decline, and the
/// cutover the primary's adapter can require of that replica.
pub(super) fn storage_backends(root: &Map<String, Value>, problems: &mut Vec<String>) {
    let primary_field = crate::capabilities::STORAGE_BACKEND_CONFIG;
    let primary = catalog_variant(
        crate::capabilities::RuntimeFacet::Storage,
        field_in(root, &primary_field),
        primary_field.path,
        problems,
    );
    if let Some(variant) = primary {
        validate_variant_config(root, variant, false, problems);
    }

    let backup_path = primary_field
        .backup_path
        .expect("storage backend catalog entry must define its backup path");
    // An empty backup backend is the documented "there is no Stado-managed
    // backup" state that `queue::copy::Endpoint::configured_backup` reads and
    // returns `None` for. Validation rejected it anyway, because the catalog
    // lookup only skips `null`, so the one decision an operator might have to
    // make about a replica — that this host should not have one — could not be
    // written down. On 2026-08-30 that host's mis-addressed replication had to
    // be stopped by pointing the backup at the primary's own store instead, so
    // the same-store guard would refuse it: a workaround standing in for a
    // setting that already existed everywhere except here. The primary keeps
    // rejecting empty, because a queue store is required and always was.
    let backup_declared = binding_in(root, primary_field.backup_path)
        .filter(|value| value.as_str().is_none_or(|name| !name.trim().is_empty()));
    let backup_variant = catalog_variant(
        crate::capabilities::RuntimeFacet::Storage,
        backup_declared,
        backup_path,
        problems,
    );
    if let Some(variant) = backup_variant {
        validate_variant_config(root, variant, true, problems);
    }

    let primary_adapter = primary.and_then(|variant| match variant.adapter {
        crate::capabilities::RuntimeAdapter::Storage(adapter) => Some(adapter),
        _ => None,
    });
    let backup_adapter = backup_variant.and_then(|variant| match variant.adapter {
        crate::capabilities::RuntimeAdapter::Storage(adapter) => Some(adapter),
        _ => None,
    });
    if let Some(required) = primary_adapter.and_then(|adapter| adapter.required_backup()) {
        if backup_adapter != Some(required) {
            problems.push(format!(
                "{} cutover requires storage.backup.backend={}; the replica is only ever read from and is never promoted automatically",
                primary.map(|variant| variant.id).unwrap_or("selected storage"),
                required.id()
            ));
        }
    }
}
