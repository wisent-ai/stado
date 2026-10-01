//! Placement feedback: what a decision actually did, written once per
//! decision and read back newest-first.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::autonomy::storage::objects::{list_record_ids, write_json};
use crate::autonomy::storage::FEEDBACK_PREFIX;
use crate::queue::{JobStorage, StorageError};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacementFeedback {
    pub decision_id: String,
    pub subject_id: String,
    pub target_id: String,
    pub observed_at: String,
    pub startup_seconds: Option<f64>,
    pub runtime_seconds: Option<f64>,
    pub realized_cost_usd: Option<f64>,
    pub succeeded: bool,
    pub failure_class: Option<String>,
}

pub async fn write_feedback(
    store: &JobStorage,
    feedback: &PlacementFeedback,
) -> Result<(), StorageError> {
    let path = format!("{FEEDBACK_PREFIX}/{}.json", feedback.decision_id);
    write_json(store, &path, feedback, true).await
}

/// Every feedback record, newest first. The policy's artifact retention
/// (`crate::autonomy::lifecycle`) is what bounds how many there are.
pub async fn list_recent_feedback(
    store: &JobStorage,
) -> Result<Vec<PlacementFeedback>, StorageError> {
    let mut blobs = store
        .list_blobs_with_meta(&format!("{FEEDBACK_PREFIX}/"))
        .await?;
    blobs.sort_by_key(|blob| std::cmp::Reverse(blob.updated));
    let mut records = Vec::with_capacity(blobs.len());
    for blob in blobs {
        let Some(raw) = store.download_text(&blob.name).await? else {
            continue;
        };
        records.push(serde_json::from_str(&raw)?);
    }
    Ok(records)
}

/// Decision ids that already carry placement feedback. The feedback object is
/// named for the decision it answers, so the names are the answer.
pub async fn list_feedback_decision_ids(
    store: &JobStorage,
) -> Result<BTreeSet<String>, StorageError> {
    list_record_ids(store, &format!("{FEEDBACK_PREFIX}/")).await
}
