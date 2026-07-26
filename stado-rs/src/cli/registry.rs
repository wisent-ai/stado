//! `stado registry validate|push|pull` — canonical registry management.

use std::path::PathBuf;

use crate::queue::{BlobBackend, GcsBackend};
use crate::targets::{bundled_registry_path, validate_registry_file};

use super::CmdError;

const REGISTRY_BUCKET: &str = "wisent-compute";
const REGISTRY_OBJECT: &str = "registry.json";
const REGISTRY_URI: &str = "gs://wisent-compute/registry.json";

fn source_path(path: Option<String>) -> PathBuf {
    path.map(PathBuf::from).unwrap_or_else(bundled_registry_path)
}

pub fn validate(path: Option<String>) -> Result<(), CmdError> {
    let source = source_path(path);
    validate_registry_file(&source).map_err(|exc| CmdError::click(exc.to_string()))?;
    println!("valid registry: {}", source.display());
    Ok(())
}

pub async fn push(path: Option<String>) -> Result<(), CmdError> {
    let source = source_path(path);
    validate_registry_file(&source).map_err(|exc| CmdError::click(exc.to_string()))?;
    let payload = std::fs::read_to_string(&source)?;
    let backend = GcsBackend::new(REGISTRY_BUCKET)
        .await
        .map_err(|exc| CmdError::click(format!("registry upload failed: {exc}")))?;
    let current = backend
        .download_text_versioned(REGISTRY_OBJECT)
        .await
        .map_err(|exc| CmdError::click(format!("registry upload failed: {exc}")))?;
    let previous_generation = current.as_ref().map(|blob| blob.version.clone()).unwrap_or_else(|| "0".to_string());
    let generation = match current {
        Some(blob) => backend
            .compare_and_swap_text(REGISTRY_OBJECT, &blob.version, &payload)
            .await
            .map_err(|exc| CmdError::click(format!("registry upload failed: {exc}")))?,
        None => {
            let created = backend
                .upload_text_if_absent(REGISTRY_OBJECT, &payload)
                .await
                .map_err(|exc| CmdError::click(format!("registry upload failed: {exc}")))?;
            if !created {
                return Err(CmdError::click("registry upload failed: concurrent create"));
            }
            backend
                .download_text_versioned(REGISTRY_OBJECT)
                .await
                .map_err(|exc| CmdError::click(format!("registry upload failed: {exc}")))?
                .ok_or_else(|| CmdError::click("registry upload verification could not read the object"))?
                .version
        }
    };
    let confirmed = backend
        .download_text_versioned(REGISTRY_OBJECT)
        .await
        .map_err(|exc| CmdError::click(format!("registry upload failed: {exc}")))?
        .ok_or_else(|| CmdError::click("registry upload verification could not read the object"))?;
    if confirmed.version != generation || confirmed.content != payload {
        return Err(CmdError::click("registry upload verification returned different bytes"));
    }
    println!(
        "pushed {} -> {REGISTRY_URI} generation={generation} replaced={previous_generation}",
        source.display()
    );
    Ok(())
}

pub async fn pull() -> Result<(), CmdError> {
    let backend = GcsBackend::new(REGISTRY_BUCKET).await?;
    let text = backend
        .download_text(REGISTRY_OBJECT)
        .await?
        .ok_or_else(|| CmdError::click("could not fetch registry from GCS"))?;
    let value: serde_json::Value = serde_json::from_str(&text)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

/// `stado registry self [--name-only]` — which registry target is this
/// machine. Installers need it: a plist that hardcodes a name the registry
/// does not carry produces a daemon that starts, fails its identity lookup
/// and exits, on every respawn, forever.
pub async fn self_target(name_only: bool) -> Result<(), CmdError> {
    let hostname = crate::providers::vast::system_hostname();
    let registry = crate::targets::load_registry_gcs().await;
    let found = registry
        .lookup_self(&hostname)
        .map_err(|exc| CmdError::click(exc.to_string()))?
        .ok_or_else(|| CmdError::click(format!("host {hostname} is not in {REGISTRY_URI}")))?;
    if name_only {
        println!("{}", found.name);
    } else {
        println!("{}\t{}\t{}", found.name, found.kind, hostname);
    }
    Ok(())
}
