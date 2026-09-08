//! Typed classification of a sealed A/B reconciliation snapshot, and the
//! re-observation of the decisions that classification produced.

use serde_json::Value;
use std::collections::HashSet;

use crate::queue::{JobStorage, StorageError};

use super::reads::snapshot_full_path;

mod families;
mod observe;
mod retained;

pub(crate) use observe::validate_reconciliation_final_state;

/// Classify destination-only lifecycle objects from a sealed B-winning A/B
/// snapshot using the queue's production job, transition, cancellation, run,
/// and retained-outcome contracts. This function is read-only by construction:
/// callers bind `store` to an immutable local checkpoint.
pub(crate) async fn classify_reconciliation_snapshot(
    store: &JobStorage,
    primary_only_paths: &[String],
) -> Result<Vec<Value>, StorageError> {
    let index = families::read_snapshot_index(store).await?;
    let mut decisions = Vec::new();
    let mut emitted_cancellations = HashSet::new();
    for relative in primary_only_paths {
        let Some((family, tail)) = relative.split_once('/') else {
            decisions.push(serde_json::json!({
                "kind": "block_unclassified_live",
                "path": snapshot_full_path(relative),
                "reason": "lifecycle object has no canonical family key",
            }));
            continue;
        };
        let full_path = snapshot_full_path(relative);
        families::classify_lifecycle_path(
            store,
            &index,
            &mut emitted_cancellations,
            &mut decisions,
            families::LifecyclePath {
                relative: relative.as_str(),
                family,
                tail,
                full_path: full_path.as_str(),
            },
        )
        .await?;
    }
    Ok(decisions)
}
