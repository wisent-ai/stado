//! Read one run manifest document, and list every run id under `runs/`.

use serde_json::{Map, Value};

use crate::queue::storage::JobStorage;
use crate::queue::StorageError;

use super::super::prefixes::RUN_PREFIX;

/// Read a run manifest; `None` when it does not exist.
pub async fn read_run(
    store: &JobStorage,
    run_id: &str,
) -> Result<Option<Map<String, Value>>, StorageError> {
    crate::queue::submit::validate_run_id(run_id)
        .map_err(|error| StorageError::Other(error.to_string()))?;
    let path = format!("{RUN_PREFIX}/{run_id}.json");
    let Some(versioned) = store.read_text_versioned(&path).await? else {
        return Ok(None);
    };
    let value: Value = serde_json::from_str(&versioned.content)?;
    let value = if value.get("schema").and_then(Value::as_str) == Some("stado.run-submission.v2") {
        match crate::queue::submit::migrate_v2_run_manifest(store, run_id).await {
            Ok(migrated) => migrated,
            Err(crate::queue::submit::SubmitError::Storage(StorageError::NotFound(missing)))
                if missing == path =>
            {
                return Ok(None);
            }
            Err(error) => return Err(StorageError::Other(error.to_string())),
        }
    } else {
        value
    };
    match value {
        Value::Object(map) => Ok(Some(map)),
        _ => Err(StorageError::Other(format!(
            "run manifest {run_id} is not a JSON object"
        ))),
    }
}

/// Run ids of all manifests under `runs/`: each `runs/<run id>.json`.
///
/// The same prefix also holds other records in directories of their own —
/// `runs/build/<product>/<build>/manifest.json`, `runs/release-pipeline/<id>/
/// run.json`, the pending changes — and the listing is recursive. Taking the
/// last segment of every path read those leaves as run ids `manifest`, `run`
/// and `changes`, once per record, so a reader of every run (`stado job
/// watch` of a reaped job, `stado machine logs`) asked the store for
/// `runs/manifest.json` thousands of times and its connection was closed
/// before it found the job. Only a manifest directly under the prefix is a
/// run.
pub async fn list_runs(store: &JobStorage) -> Result<Vec<String>, StorageError> {
    let prefix = format!("{RUN_PREFIX}/");
    let paths = store.list_paths(&prefix, 0).await?;
    Ok(paths
        .iter()
        .filter_map(|path| path.strip_prefix(&prefix))
        .filter(|name| !name.contains('/'))
        .filter_map(|name| name.strip_suffix(".json"))
        .map(str::to_string)
        .collect())
}
