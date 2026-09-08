//! The decision record: written once per placement, updated in place by
//! compare-and-swap, and indexed by the listing rather than by re-reading it.

use chrono::{DateTime, Utc};

use crate::autonomy::model::DecisionRecord;
use crate::autonomy::storage::objects::{list_record_index, load_records};
use crate::autonomy::storage::DECISION_PREFIX;
use crate::queue::{JobStorage, StorageError};

pub async fn write_decision(
    store: &JobStorage,
    decision: &DecisionRecord,
) -> Result<(), StorageError> {
    let path = decision_path(&decision.decision_id);
    let content = serde_json::to_string(decision)?;
    if store.create_text_if_absent(&path, &content).await? {
        Ok(())
    } else {
        Err(StorageError::StorageConflict(format!(
            "decision {} already exists",
            decision.decision_id
        )))
    }
}

pub async fn update_decision(
    store: &JobStorage,
    decision: &DecisionRecord,
) -> Result<(), StorageError> {
    let path = decision_path(&decision.decision_id);
    let current = store
        .read_text_versioned(&path)
        .await?
        .ok_or_else(|| StorageError::NotFound(path.clone()))?;
    store
        .compare_and_swap_text(&path, &current.version, &serde_json::to_string(decision)?)
        .await?;
    Ok(())
}

pub async fn load_decision(
    store: &JobStorage,
    decision_id: &str,
) -> Result<Option<DecisionRecord>, StorageError> {
    let Some(raw) = store.download_text(&decision_path(decision_id)).await? else {
        return Ok(None);
    };
    Ok(Some(serde_json::from_str(&raw)?))
}

pub async fn list_decisions(store: &JobStorage) -> Result<Vec<DecisionRecord>, StorageError> {
    load_records(store, &format!("{DECISION_PREFIX}/")).await
}

/// Every decision id with the moment its record was last written.
pub async fn list_decision_index(
    store: &JobStorage,
) -> Result<Vec<(String, Option<DateTime<Utc>>)>, StorageError> {
    list_record_index(store, &format!("{DECISION_PREFIX}/")).await
}

fn decision_path(decision_id: &str) -> String {
    format!("{DECISION_PREFIX}/{decision_id}.json")
}
