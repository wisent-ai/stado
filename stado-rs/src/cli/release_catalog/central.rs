use std::collections::BTreeSet;
use std::path::Path;

use crate::cli::CmdError;
use crate::release_pipeline::{self, ReleaseCatalogEntry};

use super::{product, publish_entry};

fn print_entries(entries: &[ReleaseCatalogEntry], json: bool) -> Result<(), CmdError> {
    if json {
        println!("{}", serde_json::to_string_pretty(entries)?);
    } else {
        for entry in entries {
            println!(
                "cataloged {} manifest={}",
                entry.product, entry.manifest_sha256
            );
        }
    }
    Ok(())
}

pub(super) async fn sync_catalog(path: &Path, json: bool) -> Result<(), CmdError> {
    let bytes = std::fs::read(path)?;
    let document: serde_json::Value = serde_json::from_slice(&bytes)?;
    let repositories = document
        .get("repositories")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| CmdError::click("central catalog repositories must be an array"))?;
    let mut products = BTreeSet::new();
    let mut entries = Vec::new();
    for repository in repositories {
        let repository_name = repository
            .get("repository")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<unknown repository>");
        let manifest_value = repository
            .get("manifest")
            .ok_or_else(|| CmdError::click("central catalog entry is missing manifest"))?;
        let manifest_bytes = serde_json::to_vec(manifest_value)?;
        let manifest = release_pipeline::parse_product_manifest(&manifest_bytes)
            .map_err(|error| CmdError::click(format!("{repository_name}: {error}")))?;
        let name = product(&manifest).to_string();
        if !products.insert(name.clone()) {
            return Err(CmdError::click(format!(
                "central catalog contains duplicate product {name:?}"
            )));
        }
        let declared_product = repository
            .get("product")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| CmdError::click("central catalog entry is missing product"))?;
        if declared_product != name {
            return Err(CmdError::click(format!(
                "central catalog product {declared_product:?} disagrees with manifest {name:?}"
            )));
        }
        let manifest_sha256 = repository
            .get("manifest_sha256")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| CmdError::click("central catalog entry is missing manifest_sha256"))?;
        let entry = publish_entry(manifest, manifest_sha256.to_string(), None)
            .await
            .map_err(|error| CmdError::click(format!("{repository_name}: {error}")))?;
        entries.push(entry);
    }
    if entries.is_empty() {
        return Err(CmdError::click(
            "central catalog contains no repository entries",
        ));
    }
    print_entries(&entries, json)
}
