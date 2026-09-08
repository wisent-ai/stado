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

/// Run ids of all manifests under `runs/`.
pub async fn list_runs(store: &JobStorage) -> Result<Vec<String>, StorageError> {
    let paths = store.list_paths(&format!("{RUN_PREFIX}/"), 0).await?;
    Ok(paths
        .iter()
        .filter_map(|p| p.rsplit('/').next())
        .filter(|name| name.ends_with(".json"))
        .map(|name| name[..name.len() - 5].to_string())
        .collect())
}
