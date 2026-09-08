//! One capture entry of a plan document, checked field by field against the
//! capture contract before any of it reaches a worker host.

use serde_json::{Map, Value};

use super::super::{Capture, ARTIFACT_NAMESPACE, AXES, CAPTURE_KEYS, MAX_STEPS, STEP_OPS};
use crate::deploy::DeployError;

/// One capture entry, checked field by field. `index` is one-based because it
/// appears in every refusal and an operator counts a list from one.
pub(super) fn parse_capture(
    index: usize,
    entry: &Value,
    batch: &str,
) -> Result<Capture, DeployError> {
    let object = entry
        .as_object()
        .ok_or_else(|| DeployError(format!("capture {index} is not a JSON object")))?;
    for key in object.keys() {
        if !CAPTURE_KEYS.contains(&key.as_str()) {
            return Err(DeployError(format!(
                "capture {index} carries the key {key}, which is not one of {}",
                CAPTURE_KEYS.join(", ")
            )));
        }
    }

    let site_slug = text_field(object, "site_slug");
    if site_slug.is_empty() {
        return Err(DeployError(format!(
            "capture {index} must carry a non-empty site_slug"
        )));
    }
    let source_url = text_field(object, "source_url");
    if !source_url.starts_with("http://") && !source_url.starts_with("https://") {
        return Err(DeployError(format!(
            "capture {index} source_url must be an http or https URL"
        )));
    }
    let axis = text_field(object, "axis");
    if !AXES.contains(&axis.as_str()) {
        return Err(DeployError(format!(
            "capture {index} axis must be one of {}",
            AXES.join(", ")
        )));
    }

    let viewport = object.get("viewport").and_then(Value::as_object);
    let positive = |key: &str| {
        viewport
            .and_then(|viewport| viewport.get(key))
            .and_then(Value::as_f64)
            .is_some_and(|value| value > f64::default())
    };
    if !positive("width") || !positive("height") || !positive("device_scale_factor") {
        return Err(DeployError(format!(
            "capture {index} viewport must carry a positive width, height and device_scale_factor"
        )));
    }
    if object.get("full_page").and_then(Value::as_bool).is_none() {
        return Err(DeployError(format!(
            "capture {index} full_page must be true or false"
        )));
    }
    if !object
        .get("record_seconds")
        .and_then(Value::as_f64)
        .is_some_and(|seconds| seconds >= f64::default())
    {
        return Err(DeployError(format!(
            "capture {index} record_seconds must be a number of seconds that is zero or more"
        )));
    }

    let steps = object
        .get("steps")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            DeployError(format!(
                "capture {index} steps must be an array of objects carrying op and value"
            ))
        })?;
    if steps.len() > MAX_STEPS {
        return Err(DeployError(format!(
            "capture {index} carries {} steps; Weles accepts at most {MAX_STEPS} per capture",
            steps.len()
        )));
    }
    for (position, step) in steps.iter().enumerate() {
        let op = step
            .as_object()
            .and_then(|step| step.get("op"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !STEP_OPS.contains(&op) {
            return Err(DeployError(format!(
                "capture {index} step {} names the operation {op}, which is not one of {}",
                position + 1,
                STEP_OPS.join(", ")
            )));
        }
    }

    let artifact_prefix = text_field(object, "artifact_prefix");
    let batch_root = format!("stado://{ARTIFACT_NAMESPACE}/{batch}/");
    if !artifact_prefix.starts_with(&batch_root) {
        return Err(DeployError(format!(
            "capture {index} artifact_prefix must be under {batch_root}"
        )));
    }
    if !artifact_prefix.ends_with('/') {
        return Err(DeployError(format!(
            "capture {index} artifact_prefix must end with '/'"
        )));
    }
    let key = artifact_prefix.trim_start_matches("stado://").to_string();
    if key
        .trim_end_matches('/')
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
        || key.contains('\\')
    {
        return Err(DeployError(format!(
            "capture {index} artifact_prefix must not contain an empty, '.' or '..' path segment"
        )));
    }

    // The params object reaches the worker as written, with `batch` set to the
    // batch this run resolved: `--batch` has to reach the artifact sidecars,
    // not just the enqueue report.
    let mut params = object.clone();
    params.insert("batch".to_string(), Value::from(batch));
    Ok(Capture {
        site_slug,
        axis,
        source_url,
        artifact_prefix,
        params,
    })
}

fn text_field(object: &Map<String, Value>, key: &str) -> String {
    object
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}
