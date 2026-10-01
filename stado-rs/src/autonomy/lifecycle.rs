//! Retention-aware cleanup for autonomy control-plane objects.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::queue::{JobStorage, StorageError};

use super::policy::{AutonomyMode, AutonomyPolicy};

/// What one lifecycle pass deleted, and whether the policy's own per-tick
/// byte budget stopped it before every expired record was gone.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleSummary {
    pub deleted: Vec<String>,
    pub deleted_bytes: u64,
    pub capped: bool,
}

/// Delete every record the policy declares expired. A policy that names
/// `max_deleted_bytes_per_tick` bounds one tick to that many bytes; a policy
/// that names none is not bounded.
pub async fn enforce(
    store: &JobStorage,
    policy: &AutonomyPolicy,
    now: DateTime<Utc>,
) -> Result<LifecycleSummary, StorageError> {
    let mut summary = LifecycleSummary::default();
    if policy.mode != AutonomyMode::EnforceOwned || policy.emergency_paused {
        return Ok(summary);
    }
    let max_deleted_bytes = policy.limits.max_deleted_bytes_per_tick;
    let over_budget = |summary: &LifecycleSummary, size: u64| {
        max_deleted_bytes.is_some_and(|budget| summary.deleted_bytes.saturating_add(size) > budget)
    };
    let artifact_ttl = policy.idle.artifact_days * crate::monitor::billing::SECONDS_PER_DAY;
    let targets = [
        ("state/autonomy/leases/", policy.limits.decision_ttl_seconds),
        ("state/autonomy/decisions/", artifact_ttl),
        ("state/autonomy/plans/", artifact_ttl),
        ("state/autonomy/feedback/", artifact_ttl),
    ];
    for (prefix, ttl_seconds) in targets {
        let blobs = store.list_blobs_with_meta(prefix).await?;
        for blob in blobs {
            let Some(updated) = blob.updated else {
                continue;
            };
            if now.signed_duration_since(updated).num_seconds()
                < i64::try_from(ttl_seconds).unwrap_or(i64::MAX)
            {
                continue;
            }
            let size = blob.size.unwrap_or_default();
            if over_budget(&summary, size) {
                summary.capped = true;
                continue;
            }
            store.delete_blob(&blob.name).await?;
            summary.deleted.push(blob.name.clone());
            summary.deleted_bytes = summary.deleted_bytes.saturating_add(size);
        }
    }
    let mut snapshots = store
        .list_blobs_with_meta("state/autonomy/inventory/snapshots/")
        .await?;
    snapshots.sort_by_key(|blob| std::cmp::Reverse(blob.updated));
    // The newest snapshot is kept whatever its age.
    let Some((_newest, older)) = snapshots.split_first() else {
        return Ok(summary);
    };
    for blob in older {
        let Some(updated) = blob.updated else {
            continue;
        };
        if now.signed_duration_since(updated).num_seconds()
            < i64::try_from(policy.idle.snapshot_days * crate::monitor::billing::SECONDS_PER_DAY)
                .unwrap_or(i64::MAX)
        {
            continue;
        }
        let size = blob.size.unwrap_or_default();
        if over_budget(&summary, size) {
            summary.capped = true;
            continue;
        }
        store.delete_blob(&blob.name).await?;
        summary.deleted.push(blob.name.clone());
        summary.deleted_bytes = summary.deleted_bytes.saturating_add(size);
    }
    Ok(summary)
}
