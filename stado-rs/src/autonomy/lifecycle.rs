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

/// Bytes of expired records one tick may delete when the policy names no
/// budget of its own. Sized to drain a backlog of small JSON records over
/// minutes rather than in one sweep, so a mistake stays small and visible.
const DEFAULT_MAX_DELETED_BYTES_PER_TICK: u64 = 8 * 1024 * 1024;

pub async fn enforce(
    store: &JobStorage,
    policy: &AutonomyPolicy,
    now: DateTime<Utc>,
) -> Result<LifecycleSummary, StorageError> {
    let mut summary = LifecycleSummary::default();
    if policy.mode != AutonomyMode::EnforceOwned || policy.emergency_paused {
        return Ok(summary);
    }
    // An unset per-tick delete budget used to mean "delete nothing", so the
    // one mechanism that keeps these prefixes from growing without end was off
    // unless a policy remembered to name a number. Nothing named it: on
    // 2026-09-03 the feedback prefix held 3,642 records and the decision
    // prefix 9,244, all of them past the 30-day artifact TTL this policy
    // declares, and every planning pass paid for the pile. The safety valve
    // had become the switch.
    //
    // Unset now means the default budget below, which is a bound and not a
    // licence: a tick deletes at most that many bytes of records the policy
    // already says are expired, so a backlog drains over ticks instead of in
    // one sweep. An explicit zero still means "delete nothing", which is how a
    // policy says so deliberately.
    let max_deleted_bytes = match policy.limits.max_deleted_bytes_per_tick {
        Some(budget) => budget,
        None => DEFAULT_MAX_DELETED_BYTES_PER_TICK,
    };
    if max_deleted_bytes == u64::default() {
        return Ok(summary);
    }
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
        .list_blobs_with_meta("state/autonomy/inventory/snapshots/")
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
