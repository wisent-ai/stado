//! Preparing the one config document the cutover installs everywhere.
//!
//! The document is built and validated before the first store is fenced, so a
//! config that would not pass validation costs nothing. The provider allowlist
//! is replaced outright rather than merged, the disabled list is emptied, and
//! the storage locator is written from the destination endpoint through the
//! capability catalog so no adapter's field names are hard-coded here.
//!
//! [`install`] writes those same bytes atomically, locally and remotely.

pub(super) mod install;

use std::fs;

use serde_json::{Map, Value};

use crate::cli::recovery::request::{PreparedConfig, RecoveryMigrateArgs};
use crate::cli::CmdError;
use crate::queue::copy::Endpoint;

pub(super) fn prepare_config(
    args: &RecoveryMigrateArgs,
    destination: &Endpoint,
) -> Result<PreparedConfig, CmdError> {
    let path = match &args.config {
        Some(path) => crate::config_file::expand_tilde(&path.to_string_lossy()),
        None => crate::config_file::find_config_file().ok_or_else(|| CmdError::click("no Stado config file exists; pass --config PATH so recovery can perform an explicit atomic cutover"))?,
    };
    let text = fs::read_to_string(&path)?;
    let mut document: Value = serde_json::from_str(&text)?;
    let root = document
        .as_object_mut()
        .ok_or_else(|| CmdError::click(format!("{} must contain a JSON object", path.display())))?;
    root.insert(
        "providers".to_string(),
        Value::Array(
            args.enable_providers
                .iter()
                .map(|provider| Value::String(provider.clone()))
                .collect(),
        ),
    );
    root.insert("providers_disabled".to_string(), Value::Array(Vec::new()));
    set_storage_destination(root, destination)?;
    let problems = crate::config_file::validate(&document);
    if !problems.is_empty() {
        return Err(CmdError::click(format!(
            "cutover config is invalid: {}",
            problems.join("; ")
        )));
    }
    let mut bytes = serde_json::to_vec_pretty(&document)?;
    bytes.push(b'\n');
    Ok(PreparedConfig { path, bytes })
}

fn set_storage_destination(
    root: &mut Map<String, Value>,
    destination: &Endpoint,
) -> Result<(), CmdError> {
    let storage = root
        .entry("storage".to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| CmdError::click("config storage must be an object"))?;
    let variant = crate::capabilities::constructible_variant(
        crate::capabilities::RuntimeFacet::Storage,
        &destination.kind,
    )
    .ok_or_else(|| CmdError::usage(format!("unsupported destination {:?}", destination.kind)))?;
    storage.insert("backend".to_string(), Value::String(variant.id.to_string()));
    let section = variant
        .config
        .first()
        .and_then(|field| field.path.strip_prefix("storage."))
        .and_then(|path| path.split_once('.'))
        .map(|(section, _)| section)
        .ok_or_else(|| {
            CmdError::click(format!(
                "storage catalog variant {:?} has no locator section",
                variant.id
            ))
        })?;
    let locator = storage
        .entry(section.to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| CmdError::click(format!("config storage.{section} must be an object")))?;
    for field in variant.config {
        let value = destination.locator_value(field.key).ok_or_else(|| {
            CmdError::click(format!(
                "storage catalog field {:?} has no endpoint locator",
                field.key
            ))
        })?;
        locator.insert(field.key.to_string(), Value::String(value.to_string()));
    }
    let backup_is_gcs = storage
        .get("backup")
        .and_then(Value::as_object)
        .and_then(|backup| backup.get("backend"))
        .and_then(Value::as_str)
        .and_then(|backend| {
            crate::capabilities::canonical_id(crate::capabilities::RuntimeFacet::Storage, backend)
        })
        == Some(crate::capabilities::StorageAdapter::Gcs.id());
    let destination_is_gcs = matches!(
        variant.adapter,
        crate::capabilities::RuntimeAdapter::Storage(crate::capabilities::StorageAdapter::Gcs)
    );
    if backup_is_gcs && !destination_is_gcs {
        storage.remove("backup");
    }
    Ok(())
}
