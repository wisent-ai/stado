//! Versioned configuration accessors and operational constants.
//!
//! Environment variables are limited to documented route-local overrides.
//! Deployment-wide provider, storage, identity, and policy state resolves from
//! the selected schema-versioned configuration file.

use crate::config_file::{resolve as cfg, resolve_list as cfg_list};

mod boundaries;
mod compute;
mod fleet;

pub use boundaries::*;
pub use compute::*;
pub use fleet::*;

fn resolve_binding(
    field: &crate::capabilities::ConfigField,
    backup: bool,
    default: &str,
) -> String {
    let (env, path, alternate) = if backup {
        (
            field
                .backup_env
                .expect("catalog field has no backup environment binding"),
            field
                .backup_path
                .expect("catalog field has no backup configuration path"),
            default.to_string(),
        )
    } else {
        let alternate = if field.alternate_env.is_some() || field.alternate_path.is_some() {
            cfg(
                field.alternate_env.unwrap_or(""),
                field.alternate_path.unwrap_or(""),
                default,
            )
        } else {
            default.to_string()
        };
        (field.env, field.path, alternate)
    };
    cfg(env, path, &alternate)
}

fn resolve_capability_binding(
    kind: crate::capabilities::RuntimeFacet,
    variant: &str,
    key: &str,
    backup: bool,
    default: &str,
) -> String {
    let field = crate::capabilities::config_field(kind, variant, key)
        .expect("runtime configuration binding is missing from the capability catalog");
    debug_assert_eq!(
        field.value_kind,
        crate::capabilities::ConfigValueKind::Scalar
    );
    resolve_binding(field, backup, default)
}

fn resolve_capability_list_binding(
    kind: crate::capabilities::RuntimeFacet,
    variant: &str,
    key: &str,
    default: &[&str],
) -> Vec<String> {
    let field = crate::capabilities::config_field(kind, variant, key)
        .expect("runtime list binding is missing from the capability catalog");
    debug_assert_eq!(field.value_kind, crate::capabilities::ConfigValueKind::List);
    cfg_list(field.env, field.path, default)
}

fn resolve_compute_binding(
    provider: crate::capabilities::ProviderId,
    key: &str,
    default: &str,
) -> String {
    resolve_capability_binding(
        crate::capabilities::RuntimeFacet::Compute,
        provider.as_str(),
        key,
        false,
        default,
    )
}

fn resolve_compute_list_binding(
    provider: crate::capabilities::ProviderId,
    key: &str,
    default: &[&str],
) -> Vec<String> {
    resolve_capability_list_binding(
        crate::capabilities::RuntimeFacet::Compute,
        provider.as_str(),
        key,
        default,
    )
}

fn resolve_storage_binding(
    adapter: crate::capabilities::StorageAdapter,
    key: &str,
    backup: bool,
    default: &str,
) -> String {
    resolve_capability_binding(
        crate::capabilities::RuntimeFacet::Storage,
        adapter.id(),
        key,
        backup,
        default,
    )
}

const DEFAULT_GCP_PROJECT: &str = "";
const DEFAULT_GCS_BUCKET: &str = "";
const DEFAULT_GCP_REGION: &str = "us-central1";
const DEFAULT_GCP_REGIONS: &[&str] = &[
    "us-central1",
    "europe-west4",
    "us-east1",
    "us-east4",
    "us-east5",
];
const DEFAULT_PROVIDERS: &[&str] = &[];
const DEFAULT_STORAGE_BACKEND: &str = "";

fn resolve_storage_backend(backup: bool) -> String {
    let name = resolve_binding(
        &crate::capabilities::STORAGE_BACKEND_CONFIG,
        backup,
        DEFAULT_STORAGE_BACKEND,
    );
    crate::capabilities::canonical_id(crate::capabilities::RuntimeFacet::Storage, &name)
        .unwrap_or(&name)
        .to_string()
}

fn cfg_i64(env_name: &str, dotted: &str, default: &str) -> i64 {
    cfg(env_name, dotted, default)
        .parse::<i64>()
        .unwrap_or_else(|_| panic!("{env_name} must be an integer"))
}

fn canonicalize_capability_names(
    kind: crate::capabilities::RuntimeFacet,
    values: Vec<String>,
) -> Vec<String> {
    values
        .into_iter()
        .map(|name| {
            crate::capabilities::canonical_id(kind, &name)
                .unwrap_or(&name)
                .to_string()
        })
        .collect()
}

fn canonical_machine_name(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}
