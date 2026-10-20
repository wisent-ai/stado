//! Read every Cargo execution receipt retained in a checkout's Stado build
//! area. This is the same evidence directory `execute_revision` writes, not a
//! second build index that could disagree with it.
//!
//! `execute_revision` names each receipt's directory by its invocation, a
//! UUID. The same `cargo` area also holds the installation's Cargo cache
//! (`<platform>/<lock digest>/<revision>/<recipe>`), which is no receipt;
//! read as one, its missing `result.json` refused every receipt read, and
//! with it every supervised send that reads compiler charges.
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::common::Runtime;

fn entries(area: &Path) -> Result<Vec<Value>> {
    let entries = match fs::read_dir(area) {
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
        let invocation = entry.file_name();
        if invocation
            .to_str()
            .and_then(|name| uuid::Uuid::parse_str(name).ok())
            .is_none()
        {
            continue;
        }
        let path = entry.path().join("result.json");
        // An invocation writes its result when it ends; until then, or when
        // it ended without writing one, it is listed as unfinished rather
        // than refusing every other receipt.
        let unfinished =
            "the invocation has written no result.json: it has not ended, or it ended \
                          without one";
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                results.push(json!({ "path": path, "state": "unfinished", "error": unfinished }));
                continue;
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("reading Cargo receipt {}", path.display()))
            }
        };
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
