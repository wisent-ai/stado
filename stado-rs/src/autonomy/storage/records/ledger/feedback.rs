//! Placement feedback: what a decision actually did, written once per
//! decision and read back newest-first.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::autonomy::storage::objects::{list_record_ids, write_json};
use crate::autonomy::storage::FEEDBACK_PREFIX;
use crate::queue::{JobStorage, StorageError};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacementFeedback {
    pub schema_version: u16,
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

/// The most recently written feedback records, newest first, at most `cap` of
/// them.
///
/// The reader this replaces listed the whole prefix and downloaded every body,
/// one request each. That cost is linear in everything ever written and it was
/// paid on every planning pass: on 2026-09-03 the prefix held 3,642 records,
/// so a pass issued 3,642 sequential object reads and the object API stayed
/// pinned near a full core serving them, back to back, forever. The population
/// only grows, so the pass could never get cheaper on its own.
///
/// One list call carries `updated` for every record, so the newest `cap` are
/// chosen without reading a single body, and only those bodies are fetched.
/// The bound is on the read, not on the index: nothing is skipped over and no
/// cursor is kept, so there is no position to lose and a pass never has to
/// resume where another left off.
///
/// Newest-first is also the better statistic. The only consumers are a
/// per-target median startup time and a per-target failure ratio, and a
/// target's behaviour last week describes it better than the same target
/// averaged over a month of records that a 30-day retention was always meant
/// to have deleted.
pub async fn list_recent_feedback(
    store: &JobStorage,
    cap: usize,
) -> Result<Vec<PlacementFeedback>, StorageError> {
    let mut blobs = store
        .list_blobs_with_meta(&format!("{FEEDBACK_PREFIX}/"))
        .await?;
    blobs.sort_by_key(|blob| std::cmp::Reverse(blob.updated));
    blobs.truncate(cap);
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
