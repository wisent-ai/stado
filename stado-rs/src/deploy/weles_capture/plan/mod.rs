//! The capture request as a document: reading one plan, checking what it
//! declares, and refusing it whole before the host is touched.

mod entry;

use serde_json::Value;

use super::{safe_component, Plan, PLAN_SCHEMA};
use crate::deploy::DeployError;
use entry::parse_capture;

/// Read and validate a capture plan.
///
/// `target` is the host named on the command line: the plan states which host
/// it was written for, and a mismatch is refused rather than reconciled. A
/// plan whose artifact prefixes address one host's batch is not a plan for a
/// different host.
pub fn parse_plan(path: &str, target: &str, batch: Option<&str>) -> Result<Plan, DeployError> {
    let bytes = std::fs::read(path)
        .map_err(|error| DeployError(format!("capture plan {path} cannot be read: {error}")))?;
    let document: Value = serde_json::from_slice(&bytes).map_err(|error| {
        DeployError(format!("capture plan {path} is not readable JSON: {error}"))
    })?;
    let document = document
        .as_object()
        .ok_or_else(|| DeployError(format!("capture plan {path} must be a JSON object")))?;
    if document.get("schema").and_then(Value::as_str) != Some(PLAN_SCHEMA) {
        return Err(DeployError(format!(
            "capture plan must declare the schema {PLAN_SCHEMA}"
        )));
    }

    let batch = batch
        .or_else(|| document.get("batch").and_then(Value::as_str))
        .unwrap_or_default()
        .trim()
        .to_string();
    if batch.is_empty() {
        return Err(DeployError(
            "capture plan must name a batch id, or --batch must supply one".to_string(),
        ));
    }
    safe_component("capture batch id", &batch)?;

    let declared = document
        .get("target")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if declared.is_empty() {
        return Err(DeployError(
            "capture plan must name the target host it was written for".to_string(),
        ));
    }
    if declared != target {
        return Err(DeployError(format!(
            "capture plan was written for target {declared}, not {target}"
        )));
    }

    let entries = document
        .get("captures")
        .and_then(Value::as_array)
        .ok_or_else(|| DeployError("capture plan must carry a captures array".to_string()))?;
    if entries.is_empty() {
        return Err(DeployError(
            "capture plan carries no captures, so there is nothing to enqueue".to_string(),
        ));
    }
    let mut captures = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        captures.push(parse_capture(index + 1, entry, &batch)?);
    }
    Ok(Plan {
        batch,
        target: declared.to_string(),
        captures,
    })
}
