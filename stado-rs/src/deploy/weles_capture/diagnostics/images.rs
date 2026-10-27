//! The image half of one run's browser network recording, summarized without
//! carrying a signed query string or a response body off the worker.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::primitives::tally::Tally;

use super::super::channel::Channel;
use super::diagnostic_run_id;
use super::resources::{
    is_legacy_cloud_image_url, is_stado_object_url, public_resource_url, resource_host,
    response_content_type,
};
use crate::deploy::DeployError;

/// The address a recorded network event names; an event that names none
/// is counted apart, never read as an address.
fn url_of(event: &Value) -> Option<&str> {
    event
        .get("url")
        .and_then(Value::as_str)
        .filter(|url| !url.is_empty())
}

/// Whether a response status means the image arrived: a success, or a
/// redirect the browser followed (RFC 9110 §15.3, §15.4). A number that is
/// not an HTTP status did not deliver an image.
fn delivered(status: u64) -> bool {
    match u16::try_from(status).map(reqwest::StatusCode::from_u16) {
        Ok(Ok(code)) => code.is_success() || code.is_redirection(),
        _ => false,
    }
}

/// Summarize only image request metadata from one completed Weles browser run.
///
/// Signed query strings and response bodies stay on the worker. The report
/// exposes host/path, status and counts only.
pub async fn image_diagnostics(channel: &Channel, run_id: &str) -> Result<Value, DeployError> {
    let run_id = diagnostic_run_id(run_id)?;
    let manifest_route = format!("/diagnostics/{run_id}");
    let manifest = channel.get_json(&manifest_route).await?;
    let files = manifest
        .get("files")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            DeployError::unreachable("Weles diagnostics returned no file inventory".to_string())
        })?;
    // A file that states no size cannot be checked once downloaded, so only
    // sized recordings are candidates; the largest is the full one.
    let recording = files
        .iter()
        .filter(|file| {
            file.get("path")
                .and_then(Value::as_str)
                .is_some_and(|path| path.ends_with(".inst.json"))
        })
        .filter_map(|file| file.get("bytes").and_then(Value::as_u64).map(|bytes| (bytes, file)))
        .max_by_key(|(bytes, _)| *bytes)
        .map(|(_, file)| file)
        .ok_or_else(|| {
            DeployError("Weles diagnostics contain no browser network recording".to_string())
                .stating(crate::primitives::failure::FailureCode::NotFound)
        })?;
    let recording_path = recording
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            DeployError::unreachable("Weles diagnostic recording has no path".to_string())
        })?;
    let expected_bytes = recording
        .get("bytes")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            DeployError::unreachable("Weles diagnostic recording has no byte size".to_string())
        })?;
    let encoded_path =
        url::form_urlencoded::byte_serialize(recording_path.as_bytes()).collect::<String>();
    let recording_route = format!("/diagnostics/{run_id}/file?path={encoded_path}");
    let recording_bytes = channel.get_bytes(&recording_route).await?;
    if recording_bytes.len() as u64 != expected_bytes {
        return Err(DeployError::unreachable(format!(
            "Weles diagnostic recording changed while it was read: manifest={expected_bytes} downloaded={}",
            recording_bytes.len()
        )));
    }
    let document: Value = serde_json::from_slice(&recording_bytes).map_err(|error| {
        DeployError::unreachable(format!("Weles browser recording is not JSON: {error}"))
    })?;
    let events = document
        .get("requests")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            DeployError::unreachable("Weles browser recording has no request events".to_string())
        })?;

    let unaddressed_events = events.iter().filter(|event| url_of(event).is_none()).count();
    let mut image_urls = BTreeSet::new();
    for event in events {
        let Some(url) = url_of(event) else {
            continue;
        };
        let phase = event.get("phase").and_then(Value::as_str);
        let requested_image = phase == Some("req")
            && event.get("resourceType").and_then(Value::as_str) == Some("image");
        let returned_image = phase == Some("res")
            && response_content_type(event)
                .to_ascii_lowercase()
                .starts_with("image/");
        if requested_image || returned_image || is_stado_object_url(url) {
            image_urls.insert(url.to_string());
        }
    }

    let mut response_statuses = BTreeMap::new();
    // A failure Weles recorded without its error text is reported as null:
    // the request failed, and the recording does not say why.
    let mut request_failures: BTreeMap<String, Option<String>> = BTreeMap::new();
    for event in events {
        let Some(url) = url_of(event).filter(|url| image_urls.contains(*url)) else {
            continue;
        };
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
                    .map(str::to_string);
                request_failures.insert(url.to_string(), reason);
            }
            _ => {}
        }
    }

    let mut hosts = BTreeMap::<String, u64>::new();
    let mut statuses = BTreeMap::<String, u64>::new();
    let mut stado_statuses = BTreeMap::<String, u64>::new();
    let mut failed_resources = Vec::new();
    let mut loaded_images = Vec::new();
    let mut unresolved_images = Vec::new();
    for url in &image_urls {
        if let Some(host) = resource_host(url) {
            hosts.entry(host).or_default().tally();
        }
        let stado_object = is_stado_object_url(url);
        if let Some(reason) = request_failures.get(url) {
            failed_resources.push(json!({
                "url": public_resource_url(url),
                "failure": reason,
            }));
            continue;
        }
        match response_statuses.get(url).copied() {
            Some(status) => {
                statuses.entry(status.to_string()).or_default().tally();
                if stado_object {
                    stado_statuses.entry(status.to_string()).or_default().tally();
                }
                if delivered(status) {
                    loaded_images.push(url);
                } else {
                    failed_resources.push(json!({
                        "url": public_resource_url(url),
                        "status": status,
                    }));
                }
            }
            None => unresolved_images.push(url),
        }
    }

    Ok(json!({
        "run_id": run_id,
        "recording_file": recording_path,
        "network_events": events.len(),
        "events_without_url": unaddressed_events,
        "image_requests": image_urls.len(),
        "loaded_images": loaded_images.len(),
        "failed_images": failed_resources.len(),
        "unresolved_images": unresolved_images.len(),
        "stado_object_images": image_urls.iter().filter(|url| is_stado_object_url(url)).count(),
        "legacy_cloud_images": image_urls.iter().filter(|url| is_legacy_cloud_image_url(url)).count(),
        "image_hosts": hosts,
        "status_counts": statuses,
        "stado_status_counts": stado_statuses,
        "failed_resources": failed_resources,
        // A recording that states no page errors list reports null, not none.
        "page_errors": document
            .get("pageerrors")
            .and_then(Value::as_array)
            .map(Vec::len),
    }))
}
