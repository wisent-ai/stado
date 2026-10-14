//! Read every Cargo execution receipt retained in a checkout's Stado build
//! area. This is the same evidence directory `execute_revision` writes, not a
//! second build index that could disagree with it.
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::common::Runtime;

fn entries(area: &Path) -> Result<Vec<Value>> {
    let entries = match fs::read_dir(&area) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("reading Cargo receipts under {}", area.display()))
        }
    };
    let mut results = Vec::new();
    for entry in entries {
        let entry =
            entry.with_context(|| format!("listing Cargo receipts under {}", area.display()))?;
        if !entry
            .file_type()
            .with_context(|| format!("inspecting Cargo receipt {}", entry.path().display()))?
            .is_dir()
        {
            continue;
        }
        let path = entry.path().join("result.json");
        let bytes =
            fs::read(&path).with_context(|| format!("reading Cargo receipt {}", path.display()))?;
        let report: Value = serde_json::from_slice(&bytes)
            .with_context(|| format!("decoding Cargo receipt {}", path.display()))?;
        results.push(
            json!({ "path": path, "state": report["state"], "operation": report["operation"],
            "sources": report["sources"], "evidence": report["evidence"],
            "failedOperation": report["failed_operation"], "error": report["error"] }),
        );
    }
    Ok(results)
}

pub(super) fn list(runtime: &Runtime, requested: &Path) -> Result<Value> {
    let (manifest, checkout, repository) = super::sources::manifest(runtime, requested)?;
    let area = crate::common::runs::checkout_area(&runtime.home, &checkout)?.join("cargo");
    let results = entries(&area)?;
    Ok(json!({ "repository": repository, "manifest": manifest, "results": results }))
}

/// Every canonical checkout in the selected workspace, without opening a
/// manifest or starting Cargo. The retained receipt remains the only source
/// of state, location and pinned revision.
pub(super) fn all(runtime: &Runtime) -> Result<Value> {
    let workspace = runtime.workspace.canonicalize().with_context(|| {
        format!(
            "reading canonical workspace {}",
            runtime.workspace.display()
        )
    })?;
    let mut results = Vec::new();
    for entry in fs::read_dir(&workspace)
        .with_context(|| format!("listing canonical checkouts under {}", workspace.display()))?
    {
        let entry = entry.with_context(|| {
            format!("listing canonical checkouts under {}", workspace.display())
        })?;
        if !entry
            .file_type()
            .with_context(|| format!("inspecting checkout {}", entry.path().display()))?
            .is_dir()
        {
            continue;
        }
        let area = crate::common::runs::checkout_area(&runtime.home, &entry.path())?.join("cargo");
        results.extend(entries(&area)?);
    }
    results.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    Ok(json!({ "workspace": workspace, "results": results }))
}
