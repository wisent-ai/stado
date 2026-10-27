//! Retention-aware cleanup for autonomy control-plane objects.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::queue::{BlobInfo, JobStorage, StorageError};

use super::policy::{AutonomyMode, AutonomyPolicy};

/// What one lifecycle pass deleted, whether the policy's own per-tick byte
/// budget stopped it before every expired record was gone, and which
/// expired records it left because the store reported no size for them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleSummary {
    pub deleted: Vec<String>,
    pub deleted_bytes: u64,
    pub capped: bool,
    /// Expired records the store listed without a size. The byte budget
    /// and the deleted-bytes total count what the store measured, so a
    /// record it did not measure stays until a listing reports its size.
    pub unmeasured: Vec<String>,
}

/// Why an expired record was not deleted in this tick.
enum Kept {
    Fresh,
    Unmeasured,
    OverBudget,
}

/// Delete every record the policy declares expired. A policy that names
/// `max_deleted_bytes_per_tick` bounds one tick to that many bytes; a policy
/// that names none is not bounded.
/// TTL conversion and byte accounting must remain representable before mutation.
/// Failures log the operation, target and acknowledged progress before propagating unchanged.
pub async fn enforce(
    store: &JobStorage,
    policy: &AutonomyPolicy,
    now: DateTime<Utc>,
    log: &dyn Fn(&str),
) -> Result<LifecycleSummary, StorageError> {
    let mut summary = LifecycleSummary::default();
    if policy.mode != AutonomyMode::EnforceOwned || policy.emergency_paused {
        return Ok(summary);
    }
    let artifact_ttl = retention_ttl(policy.idle.artifact_days, "idle.artifact_days")
        .map_err(|error| failure(error, "validate TTL", "idle.artifact_days", &summary, log))?;
    let snapshot_ttl = retention_ttl(policy.idle.snapshot_days, "idle.snapshot_days")
        .map_err(|error| failure(error, "validate TTL", "idle.snapshot_days", &summary, log))?;
    let targets = [
        ("state/autonomy/leases/", policy.limits.decision_ttl_seconds),
        ("state/autonomy/decisions/", artifact_ttl),
        ("state/autonomy/plans/", artifact_ttl),
        ("state/autonomy/feedback/", artifact_ttl),
    ];
    for (prefix, ttl_seconds) in targets {
        let blobs = store
            .list_blobs_with_meta(prefix)
            .await
            .map_err(|error| failure(error, "list", prefix, &summary, log))?;
        for blob in blobs {
            expire(store, policy, now, &mut summary, &blob, ttl_seconds, log).await?;
        }
    }
    let snapshot_prefix = "state/autonomy/inventory/snapshots/";
    let mut snapshots = store
        .list_blobs_with_meta(snapshot_prefix)
        .await
        .map_err(|error| failure(error, "list", snapshot_prefix, &summary, log))?;
    snapshots.sort_by_key(|blob| std::cmp::Reverse(blob.updated));
    // The newest snapshot is kept whatever its age.
    let Some((_newest, older)) = snapshots.split_first() else {
        return Ok(summary);
    };
    for blob in older {
        expire(store, policy, now, &mut summary, blob, snapshot_ttl, log).await?;
    }
    Ok(summary)
}

fn retention_ttl(days: u64, field: &str) -> Result<u64, StorageError> {
    days.checked_mul(crate::monitor::billing::SECONDS_PER_DAY)
        .ok_or_else(|| {
            StorageError::Other(format!(
                "cannot convert {field}={days} days to retention seconds: value exceeds the supported range"
            ))
        })
}

fn failure(
    error: StorageError,
    operation: &str,
    target: &str,
    summary: &LifecycleSummary,
    log: &dyn Fn(&str),
) -> StorageError {
    log(&format!(
        "autonomy lifecycle failed: operation={operation:?} target={target:?} cause={error}; acknowledged={summary:?}"
    ));
    error
}

/// Delete one record if it is older than `ttl_seconds` and the tick's byte
/// budget admits it, recording in the summary why it stayed otherwise.
async fn expire(
    store: &JobStorage,
    policy: &AutonomyPolicy,
    now: DateTime<Utc>,
    summary: &mut LifecycleSummary,
    blob: &BlobInfo,
    ttl_seconds: u64,
    log: &dyn Fn(&str),
) -> Result<(), StorageError> {
    let size = match kept(policy, now, summary, blob, ttl_seconds) {
        Ok(size) => size,
        Err(Kept::Fresh) => return Ok(()),
        Err(Kept::Unmeasured) => {
            summary.unmeasured.push(blob.name.clone());
            return Ok(());
        }
        Err(Kept::OverBudget) => {
            summary.capped = true;
            return Ok(());
        }
    };
    let deleted_bytes = summary
        .deleted_bytes
        .checked_add(size)
        .ok_or_else(|| {
            StorageError::Other(format!(
                "retention refused deletion of {:?}: deleted-byte total {} plus listed size {size} is unrepresentable; no delete was attempted",
                blob.name, summary.deleted_bytes
            ))
        })
        .map_err(|error| failure(error, "account bytes", &blob.name, summary, log))?;
    store
        .delete_blob(&blob.name)
        .await
        .map_err(|error| {
            failure(error, "delete request (outcome unconfirmed)", &blob.name, summary, log)
        })?;
    summary.deleted.push(blob.name.clone());
    summary.deleted_bytes = deleted_bytes;
    Ok(())
}

/// The size a record would free, or why it stays. A record the store lists
/// without an update time, or stamped later than `now`, has no age and is
/// fresh; a record without a size cannot be counted against the budget.
fn kept(
    policy: &AutonomyPolicy,
    now: DateTime<Utc>,
    summary: &LifecycleSummary,
    blob: &BlobInfo,
    ttl_seconds: u64,
) -> Result<u64, Kept> {
    let Some(updated) = blob.updated else {
        return Err(Kept::Fresh);
    };
    let age_seconds = match u64::try_from(now.signed_duration_since(updated).num_seconds()) {
        Ok(age) => age,
        Err(_stamped_after_now) => return Err(Kept::Fresh),
    };
    if age_seconds < ttl_seconds {
        return Err(Kept::Fresh);
    }
    let Some(size) = blob.size else {
        return Err(Kept::Unmeasured);
    };
    let over_budget = policy.limits.max_deleted_bytes_per_tick.is_some_and(|budget| {
        budget
            .checked_sub(summary.deleted_bytes)
            .is_none_or(|remaining| size > remaining)
    });
    if over_budget {
        return Err(Kept::OverBudget);
    }
    Ok(size)
}
