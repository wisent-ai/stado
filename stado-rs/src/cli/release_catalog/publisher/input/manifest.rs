//! The product's release manifest as `pin-input` reads and writes it: the
//! product it names, and the input entry it gains.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::cli::CmdError;

use super::MANIFEST;

/// The release manifest of `checkout`: its path, its document and the
/// product it names. A manifest that cannot be read, is not JSON, names no
/// product or holds `inputs` that is not an object is refused by name.
pub(super) fn read(checkout: &Path) -> Result<(PathBuf, Value, String), CmdError> {
    let manifest_path = checkout.join(MANIFEST);
    let text = std::fs::read_to_string(&manifest_path).map_err(|error| {
        CmdError::click(format!("{}: {error}", manifest_path.display()))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    let manifest: Value = serde_json::from_str(&text).map_err(|error| {
        CmdError::click(format!("{}: {error}", manifest_path.display()))
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    let product = manifest["product"]
        .as_str()
        .ok_or_else(|| {
            CmdError::click(format!("{} names no product", manifest_path.display()))
                .stating(crate::primitives::failure::FailureCode::Config)
        })?
        .to_string();
    if manifest
        .get("inputs")
        .is_some_and(|inputs| !inputs.is_object())
    {
        return Err(CmdError::usage(format!(
            "{}: inputs is not an object",
            manifest_path.display()
        )));
    }
    Ok((manifest_path, manifest, product))
}

/// Record `entry` as the input `name` of `manifest` and write the manifest
/// back to `manifest_path` atomically.
pub(super) fn record(
    manifest_path: &Path,
    manifest: &mut Value,
    name: &str,
    entry: Value,
) -> Result<(), CmdError> {
    let document = manifest.as_object_mut().ok_or_else(|| {
        CmdError::click(format!("{} is not a JSON object", manifest_path.display()))
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    if !document.contains_key("inputs") {
        document.insert("inputs".to_string(), json!({}));
    }
    document["inputs"]
        .as_object_mut()
        .ok_or_else(|| {
            CmdError::click(format!(
                "{}: inputs is not an object",
                manifest_path.display()
            ))
            .stating(crate::primitives::failure::FailureCode::Config)
        })?
        .insert(name.to_string(), entry);
    stado_product::common::atomic_json(manifest_path, manifest).map_err(|error| {
        CmdError::click(format!(
            "cannot write {}: {error:#}",
            manifest_path.display()
        ))
    })
}
