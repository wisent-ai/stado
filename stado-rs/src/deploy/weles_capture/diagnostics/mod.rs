//! The recording and artefact handling: what one completed Weles run left
//! behind, and the exact bytes of it this side is allowed to read.

mod images;
mod resources;

use serde_json::Value;

use super::channel::Channel;
use crate::deploy::DeployError;

pub use images::image_diagnostics;

fn diagnostic_run_id(run_id: &str) -> Result<&str, DeployError> {
    if run_id.is_empty()
        || run_id.len() > 128
        || !run_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(DeployError(
            "Weles diagnostic run id contains an unsafe character".to_string(),
        ));
    }
    Ok(run_id)
}

/// Read the authenticated artifact inventory for one completed Weles run.
pub async fn run_diagnostics(channel: &Channel, run_id: &str) -> Result<Value, DeployError> {
    let run_id = diagnostic_run_id(run_id)?;
    channel.get_json(&format!("/diagnostics/{run_id}")).await
}

/// Download one exact artifact named by that run's authenticated inventory.
pub async fn run_diagnostic_file(
    channel: &Channel,
    run_id: &str,
    requested_path: &str,
) -> Result<Vec<u8>, DeployError> {
    const MAX_DIAGNOSTIC_FILE_BYTES: u64 = 16 * 1024 * 1024;

    let run_id = diagnostic_run_id(run_id)?;
    let manifest = run_diagnostics(channel, run_id).await?;
    let expected_bytes = manifest
        .get("files")
        .and_then(Value::as_array)
        .and_then(|files| {
            files.iter().find_map(|file| {
                (file.get("path").and_then(Value::as_str) == Some(requested_path))
                    .then(|| file.get("bytes").and_then(Value::as_u64))
                    .flatten()
            })
        })
        .ok_or_else(|| {
            DeployError(format!(
                "Weles diagnostics for {run_id} contain no file {requested_path:?}"
            ))
        })?;
    if expected_bytes > MAX_DIAGNOSTIC_FILE_BYTES {
        return Err(DeployError(format!(
            "Weles diagnostic file is {expected_bytes} bytes; this reader accepts at most {MAX_DIAGNOSTIC_FILE_BYTES}"
        )));
    }
    let encoded_path =
        url::form_urlencoded::byte_serialize(requested_path.as_bytes()).collect::<String>();
    let bytes = channel
        .get_bytes(&format!("/diagnostics/{run_id}/file?path={encoded_path}"))
        .await?;
    if bytes.len() as u64 != expected_bytes {
        return Err(DeployError(format!(
            "Weles diagnostic file changed while it was read: manifest={expected_bytes} downloaded={}",
            bytes.len()
        )));
    }
    Ok(bytes)
}
