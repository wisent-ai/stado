//! The image half of one run's browser network recording, summarized without
//! carrying a signed query string or a response body off the worker.

use serde_json::{json, Value};

use super::super::channel::Channel;
use super::diagnostic_run_id;
use super::resources::{
    is_legacy_cloud_image_url, is_stado_object_url, public_resource_url, resource_host,
    response_content_type,
};
use crate::deploy::DeployError;

/// Summarize only image request metadata from one completed Weles browser run.
///
/// Signed query strings and response bodies stay on the worker. The report
/// exposes host/path, status and counts only.
pub async fn image_diagnostics(channel: &Channel, run_id: &str) -> Result<Value, DeployError> {
    const MAX_DIAGNOSTIC_BYTES: u64 = 128 * 1024 * 1024;

    let run_id = diagnostic_run_id(run_id)?;
    let manifest_route = format!("/diagnostics/{run_id}");
    let manifest = channel.get_json(&manifest_route).await?;
    let files = manifest
        .get("files")
        .and_then(Value::as_array)
        .ok_or_else(|| DeployError("Weles diagnostics returned no file inventory".to_string()))?;
    let recording = files
        .iter()
        .filter(|file| {
            file.get("path")
                .and_then(Value::as_str)
                .is_some_and(|path| path.ends_with(".inst.json"))
        })
        .max_by_key(|file| {
            file.get("bytes")
                .and_then(Value::as_u64)
                .unwrap_or_default()
        })
        .ok_or_else(|| {
            DeployError("Weles diagnostics contain no browser network recording".to_string())
        })?;
    let recording_path = recording
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| DeployError("Weles diagnostic recording has no path".to_string()))?;
    let expected_bytes = recording
        .get("bytes")
        .and_then(Value::as_u64)
        .ok_or_else(|| DeployError("Weles diagnostic recording has no byte size".to_string()))?;
    if expected_bytes > MAX_DIAGNOSTIC_BYTES {
        return Err(DeployError(format!(
            "Weles browser network recording is {expected_bytes} bytes; the read-only image inspector accepts at most {MAX_DIAGNOSTIC_BYTES}"
        )));
    }
    let encoded_path =
        url::form_urlencoded::byte_serialize(recording_path.as_bytes()).collect::<String>();
    let recording_route = format!("/diagnostics/{run_id}/file?path={encoded_path}");
    let recording_bytes = channel.get_bytes(&recording_route).await?;
    if recording_bytes.len() as u64 != expected_bytes {
        return Err(DeployError(format!(
            "Weles diagnostic recording changed while it was read: manifest={expected_bytes} downloaded={}",
            recording_bytes.len()
        )));
    }
    let document: Value = serde_json::from_slice(&recording_bytes)
        .map_err(|error| DeployError(format!("Weles browser recording is not JSON: {error}")))?;
    let events = document
        .get("requests")
        .and_then(Value::as_array)
        .ok_or_else(|| DeployError("Weles browser recording has no request events".to_string()))?;

    let mut image_urls = std::collections::BTreeSet::new();
    for event in events {
        let url = event.get("url").and_then(Value::as_str).unwrap_or_default();
        if url.is_empty() {
            continue;
        }
        let requested_image = event.get("phase").and_then(Value::as_str) == Some("req")
            && event.get("resourceType").and_then(Value::as_str) == Some("image");
        let returned_image = event.get("phase").and_then(Value::as_str) == Some("res")
            && response_content_type(event)
                .to_ascii_lowercase()
                .starts_with("image/");
        if requested_image || returned_image || is_stado_object_url(url) {
            image_urls.insert(url.to_string());
        }
    }

    let mut response_statuses = std::collections::BTreeMap::new();
    let mut request_failures = std::collections::BTreeMap::new();
    for event in events {
        let url = event.get("url").and_then(Value::as_str).unwrap_or_default();
        if !image_urls.contains(url) {
            continue;
        }
        match event.get("phase").and_then(Value::as_str) {
            Some("res") => {
                if let Some(status) = event.get("status").and_then(Value::as_u64) {
                    response_statuses.insert(url.to_string(), status);
                }
            }
            Some("reqfailed") => {
                let reason = event
                    .get("failure")
                    .and_then(|failure| {
                        failure
                            .get("errorText")
                            .or_else(|| failure.get("error_text"))
                    })
                    .and_then(Value::as_str)
                    .unwrap_or("request failed");
                request_failures.insert(url.to_string(), reason.to_string());
            }
            _ => {}
        }
    }

    let mut hosts = std::collections::BTreeMap::<String, u64>::new();
    let mut statuses = std::collections::BTreeMap::<String, u64>::new();
    let mut stado_statuses = std::collections::BTreeMap::<String, u64>::new();
    let mut failed_resources = Vec::new();
    let mut loaded_images = 0_u64;
    let mut unresolved_images = 0_u64;
    let mut stado_object_images = 0_u64;
    let mut legacy_cloud_images = 0_u64;
    for url in &image_urls {
        if let Some(host) = resource_host(url) {
            *hosts.entry(host).or_default() += 1;
        }
        let stado_object = is_stado_object_url(url);
        if stado_object {
            stado_object_images += 1;
        }
        if is_legacy_cloud_image_url(url) {
            legacy_cloud_images += 1;
        }
        if let Some(reason) = request_failures.get(url) {
            failed_resources.push(json!({
                "url": public_resource_url(url),
                "failure": reason,
            }));
            continue;
        }
        match response_statuses.get(url).copied() {
            Some(status) => {
                *statuses.entry(status.to_string()).or_default() += 1;
                if stado_object {
                    *stado_statuses.entry(status.to_string()).or_default() += 1;
                }
                if (200..400).contains(&status) {
                    loaded_images += 1;
                } else {
                    failed_resources.push(json!({
                        "url": public_resource_url(url),
                        "status": status,
                    }));
                }
            }
            None => unresolved_images += 1,
        }
    }

    Ok(json!({
        "run_id": run_id,
        "recording_file": recording_path,
        "network_events": events.len(),
        "image_requests": image_urls.len(),
        "loaded_images": loaded_images,
        "failed_images": failed_resources.len(),
        "unresolved_images": unresolved_images,
        "stado_object_images": stado_object_images,
        "legacy_cloud_images": legacy_cloud_images,
        "image_hosts": hosts,
        "status_counts": statuses,
        "stado_status_counts": stado_statuses,
        "failed_resources": failed_resources,
        "page_errors": document
            .get("pageerrors")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
    }))
}
