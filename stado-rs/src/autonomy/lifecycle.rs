//! Retention-aware cleanup for autonomy control-plane objects.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::queue::{JobStorage, StorageError};

use super::policy::{AutonomyMode, AutonomyPolicy};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleSummary {
    pub scanned: usize,
    pub deleted: usize,
    pub deleted_bytes: u64,
    pub capped: bool,
}

pub async fn enforce(
    store: &JobStorage,
    policy: &AutonomyPolicy,
    now: DateTime<Utc>,
) -> Result<LifecycleSummary, StorageError> {
    let mut summary = LifecycleSummary::default();
    if policy.mode != AutonomyMode::EnforceOwned || policy.emergency_paused {
        return Ok(summary);
    }
    let Some(max_deleted_bytes) = policy.limits.max_deleted_bytes_per_tick else {
        return Ok(summary);
    };
    if max_deleted_bytes == u64::default() {
        return Ok(summary);
    }
    let artifact_ttl = policy.idle.artifact_days * crate::monitor::billing::SECONDS_PER_DAY;
    let targets = [
        ("autonomy/leases/", policy.limits.decision_ttl_seconds),
        ("autonomy/decisions/", artifact_ttl),
        ("autonomy/plans/", artifact_ttl),
        ("autonomy/feedback/", artifact_ttl),
    ];
    for (prefix, ttl_seconds) in targets {
        let blobs = store.list_blobs_with_meta(prefix).await?;
        for blob in blobs {
            summary.scanned += true as usize;
            let Some(updated) = blob.updated else {
                continue;
            };
            if now.signed_duration_since(updated).num_seconds()
                < i64::try_from(ttl_seconds).unwrap_or(i64::MAX)
            {
                continue;
            }
            let size = blob.size.unwrap_or_default();
            if summary.deleted_bytes.saturating_add(size) > max_deleted_bytes {
                summary.capped = true;
                continue;
            }
            store.delete_blob(&blob.name).await?;
            summary.deleted += true as usize;
            summary.deleted_bytes = summary.deleted_bytes.saturating_add(size);
        }
    }
    let mut snapshots = store
        .list_blobs_with_meta("autonomy/inventory/snapshots/")
        .await?;
    snapshots.sort_by_key(|blob| std::cmp::Reverse(blob.updated));
    for blob in snapshots.into_iter().skip(true as usize) {
        summary.scanned += true as usize;
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
        if summary.deleted_bytes.saturating_add(size) > max_deleted_bytes {
            summary.capped = true;
            continue;
        }
        store.delete_blob(&blob.name).await?;
        summary.deleted += true as usize;
        summary.deleted_bytes = summary.deleted_bytes.saturating_add(size);
    }
    Ok(summary)
}
