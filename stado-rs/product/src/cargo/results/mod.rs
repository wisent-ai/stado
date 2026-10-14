//! Read every Cargo execution receipt retained in a checkout's Stado build
//! area. This is the same evidence directory `execute_revision` writes, not a
//! second build index that could disagree with it.
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::common::Runtime;

pub(super) fn list(runtime: &Runtime, requested: &Path) -> Result<Value> {
    let (manifest, checkout, repository) = super::sources::manifest(runtime, requested)?;
    let area = crate::common::runs::checkout_area(&runtime.home, &checkout)?.join("cargo");
    let entries = match fs::read_dir(&area) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(json!({ "repository": repository, "manifest": manifest, "results": [] }));
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
    results.sort_by(|left, right| left["path"].as_str().cmp(&right["path"].as_str()));
    Ok(json!({ "repository": repository, "manifest": manifest, "results": results }))
}
