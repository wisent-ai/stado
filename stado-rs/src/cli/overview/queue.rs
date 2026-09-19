//! The queue section: how many jobs sit in each state, and the billing
//! snapshot the monitor publishes.
//!
//! Both reads answer the same question — what the store already says — so
//! they share one component: the counts come from the state prefixes and the
//! money comes from the blob the billing monitor last published.

use serde_json::{json, Map, Value};

use crate::monitor::billing;
use crate::queue::runs;
use crate::queue::{JobStorage, StorageError};

pub(super) async fn queue_counts(store: &JobStorage) -> Result<Value, StorageError> {
    let mut counts = Map::new();
    // Every prefix but `cancelled`: an overview counts work, and a job the
    // operator cancelled is not work waiting on anyone.
    for state in [
        runs::QUEUE,
        runs::RUNNING,
        runs::COMPLETED,
        runs::UPLOADED,
        runs::FAILED,
    ] {
        let prefix = format!("{state}/");
        let count = store
            .list_blobs_with_meta(&prefix)
            .await?
            .into_iter()
            .filter(|blob| {
                blob.name
                    .strip_prefix(&prefix)
                    .is_some_and(|name| name.ends_with(".json") && !name.contains('/'))
            })
            .count();
        counts.insert(state.to_string(), json!(count));
    }
    Ok(Value::Object(counts))
}

pub(super) async fn read_billing(store: &JobStorage) -> Result<Value, StorageError> {
    let Some(text) = store.download_text(billing::BLOB).await? else {
        return Ok(json!({
            "status": "unavailable",
            "detail": format!("{} has not been published yet", billing::BLOB),
        }));
    };
    serde_json::from_str(&text).map_err(StorageError::from)
}
