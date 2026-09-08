//! One diagnostics pass: upload the payload through [`JobStorage`], with the
//! local standby copy written when the upload fails.

use serde_json::Value;

use super::collect::collect;
use super::runner::{CommandRunner, SystemRunner};
use super::{LOCAL_STANDBY_PATH, OUT_PREFIX};
use crate::models::json_dumps_pretty_sorted;
use crate::queue::{JobStorage, StorageError};

/// Python `_write_local`: local standby copy on upload failure.
fn write_local(payload: &Value) {
    let text = json_dumps_pretty_sorted(payload);
    if let Err(err) = std::fs::write(LOCAL_STANDBY_PATH, text) {
        tracing::warn!("could not write {LOCAL_STANDBY_PATH}: {err}");
    }
}

/// Upload the payload through an explicit store (test seam).
pub async fn upload_with(store: &JobStorage, payload: &Value) -> Result<(), StorageError> {
    let host = payload
        .get("host")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let text = json_dumps_pretty_sorted(payload);
    store
        .upload_text(&format!("{OUT_PREFIX}/{host}.json"), &text)
        .await?;
    store
        .upload_text(&format!("{OUT_PREFIX}/{host}/latest.json"), &text)
        .await?;
    Ok(())
}

/// Python `_upload`: construct the storage handle for `bucket` and upload.
/// Construction failures count as upload failures (the Python code builds
/// the storage client inside `_upload` too).
async fn upload(bucket: &str, payload: &Value) -> Result<(), StorageError> {
    let store = JobStorage::with_bucket(bucket).await?;
    upload_with(&store, payload).await
}

/// Python `once`: collect -> upload -> print. Returns the process exit code
/// for this pass (0 uploaded, 1 upload failed and the local standby copy was
/// written). `store` is the test seam; `None` constructs it from `bucket`.
pub async fn once_with(
    bucket: &str,
    runner: &dyn CommandRunner,
    store: Option<&JobStorage>,
) -> i32 {
    let mut payload = collect(bucket, runner);
    let reported_at = payload
        .get("reported_at")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let host = payload
        .get("host")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let result = match store {
        Some(store) => upload_with(store, &payload).await,
        None => upload(bucket, &payload).await,
    };
    match result {
        Ok(()) => {
            println!("{reported_at} uploaded {OUT_PREFIX}/{host}.json");
            0
        }
        Err(err) => {
            payload["upload_error"] = Value::from(err.to_string());
            write_local(&payload);
            println!("{reported_at} upload failed; wrote {LOCAL_STANDBY_PATH}");
            1
        }
    }
}

/// Production single pass (system subprocesses, storage from config).
pub async fn once(bucket: &str) -> i32 {
    once_with(bucket, &SystemRunner, None).await
}
