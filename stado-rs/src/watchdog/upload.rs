//! One diagnostics pass: upload the payload through [`JobStorage`], and keep
//! a standby copy under the Stado home when the upload fails.

use std::path::PathBuf;

use serde_json::Value;

use super::collect::collect;
use super::runner::{CommandRunner, SystemRunner};
use super::OUT_PREFIX;
use crate::models::json_dumps_pretty_sorted;
use crate::queue::{JobStorage, StorageError};

/// A text field the collected payload must carry. Read as empty, a payload
/// without its host would be uploaded as `box_diagnostics/.json`, over every
/// other host's that also lacks one.
fn stated<'a>(payload: &'a Value, field: &str) -> Result<&'a str, String> {
    match payload.get(field).and_then(Value::as_str) {
        Some("") | None => Err(format!("the collected diagnostics carry no {field}")),
        Some(value) => Ok(value),
    }
}

/// Keep the payload of a failed upload as `~/.stado/box_diagnostics/<host>.json`:
/// the user's own directory, one file per host, where no other user or
/// host's watchdog writes over it.
fn keep_standby(host: &str, payload: &Value) -> Result<PathBuf, String> {
    let folder = crate::config_file::home()?.join(".stado").join(OUT_PREFIX);
    std::fs::create_dir_all(&folder)
        .map_err(|error| format!("cannot create {}: {error}", folder.display()))?;
    let path = folder.join(format!("{host}.json"));
    std::fs::write(&path, json_dumps_pretty_sorted(payload))
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    Ok(path)
}

/// Upload `host`'s payload through an explicit store.
pub async fn upload_with(
    store: &JobStorage,
    host: &str,
    payload: &Value,
) -> Result<(), StorageError> {
    let text = json_dumps_pretty_sorted(payload);
    store
        .upload_text(&format!("{OUT_PREFIX}/{host}.json"), &text)
        .await?;
    store
        .upload_text(&format!("{OUT_PREFIX}/{host}/latest.json"), &text)
        .await?;
    Ok(())
}

/// Construct the storage handle for `bucket` and upload; a handle that
/// cannot be built is a failed upload.
async fn upload(bucket: &str, host: &str, payload: &Value) -> Result<(), StorageError> {
    let store = JobStorage::with_bucket(bucket).await?;
    upload_with(&store, host, payload).await
}

/// How one pass ended. What it uploaded, or why it failed and where its
/// standby copy was kept, is printed by the pass itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    Uploaded,
    Failed,
}

impl std::fmt::Display for Pass {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Pass::Uploaded => "success",
            Pass::Failed => "failure",
        })
    }
}

/// Collect and upload once. `Ok` says what was uploaded; `Err` says why the
/// pass failed and where its standby copy was kept, or why none was.
async fn attempt(
    bucket: &str,
    runner: &dyn CommandRunner,
    store: Option<&JobStorage>,
) -> Result<String, String> {
    let mut payload = collect(bucket, runner);
    let reported_at = stated(&payload, "reported_at")?.to_string();
    let host = stated(&payload, "host")?.to_string();
    let result = match store {
        Some(store) => upload_with(store, &host, &payload).await,
        None => upload(bucket, &host, &payload).await,
    };
    match result {
        Ok(()) => Ok(format!("{reported_at} uploaded {OUT_PREFIX}/{host}.json")),
        Err(error) => {
            payload["upload_error"] = Value::from(error.to_string());
            let kept = keep_standby(&host, &payload).map_err(|standby| {
                format!(
                    "{reported_at} upload failed ({error}) and no standby copy was kept: {standby}"
                )
            })?;
            Err(format!(
                "{reported_at} upload failed ({error}); standby copy kept at {}",
                kept.display()
            ))
        }
    }
}

/// One pass, printed as it ended: what was uploaded on stdout, why it failed
/// on stderr. `store` is the caller's storage; `None` constructs it from
/// `bucket`.
pub async fn once_with(
    bucket: &str,
    runner: &dyn CommandRunner,
    store: Option<&JobStorage>,
) -> Pass {
    match attempt(bucket, runner, store).await {
        Ok(said) => {
            println!("{said}");
            Pass::Uploaded
        }
        Err(why) => {
            eprintln!("{why}");
            Pass::Failed
        }
    }
}

/// One pass with system subprocesses and storage from configuration.
pub async fn once(bucket: &str) -> Pass {
    once_with(bucket, &SystemRunner, None).await
}
