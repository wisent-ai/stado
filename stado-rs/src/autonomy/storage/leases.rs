//! The placement lease: one object per subject, acquired, renewed and
//! released by compare-and-swap so two coordinators never place the same
//! subject twice.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::autonomy::model::SCHEMA_VERSION;
use crate::queue::{JobStorage, StorageError};

use super::LEASE_PREFIX;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementLease {
    pub schema_version: u16,
    pub subject_id: String,
    pub decision_id: String,
    pub token: String,
    pub holder: String,
    pub acquired_at: String,
    pub expires_at: String,
}

impl PlacementLease {
    pub fn active_at(&self, now: DateTime<Utc>) -> bool {
        DateTime::parse_from_rfc3339(&self.expires_at)
            .map(|stamp| stamp.with_timezone(&Utc) > now)
            .unwrap_or(false)
    }
}

pub async fn acquire_placement_lease(
    store: &JobStorage,
    subject_id: &str,
    decision_id: &str,
    holder: &str,
    ttl_seconds: u64,
    now: DateTime<Utc>,
) -> Result<Option<PlacementLease>, StorageError> {
    let ttl_seconds = i64::try_from(ttl_seconds)
        .map_err(|_| StorageError::Other("placement lease TTL exceeds i64".to_string()))?;
    let path = lease_path(subject_id);
    let lease = PlacementLease {
        schema_version: SCHEMA_VERSION,
        subject_id: subject_id.to_string(),
        decision_id: decision_id.to_string(),
        token: uuid::Uuid::new_v4().to_string(),
        holder: holder.to_string(),
        acquired_at: now.to_rfc3339(),
        expires_at: (now + Duration::seconds(ttl_seconds)).to_rfc3339(),
    };
    let content = serde_json::to_string(&lease)?;
    if store.create_text_if_absent(&path, &content).await? {
        return Ok(Some(lease));
    }
    let Some(current) = store.read_text_versioned(&path).await? else {
        return Ok(None);
    };
    let prior: PlacementLease = serde_json::from_str(&current.content)?;
    if prior.schema_version != SCHEMA_VERSION {
        return Err(StorageError::Other(format!(
            "unsupported placement lease schema_version {}",
            prior.schema_version
        )));
    }
    if prior.active_at(now) {
        return Ok((prior.decision_id == decision_id && prior.holder == holder).then_some(prior));
    }
    match store
        .compare_and_swap_text(&path, &current.version, &content)
        .await
    {
        Ok(_) => Ok(Some(lease)),
        Err(StorageError::StorageConflict(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

pub async fn renew_placement_lease(
    store: &JobStorage,
    subject_id: &str,
    token: &str,
    ttl_seconds: u64,
    now: DateTime<Utc>,
) -> Result<Option<PlacementLease>, StorageError> {
    let ttl_seconds = i64::try_from(ttl_seconds)
        .map_err(|_| StorageError::Other("placement lease TTL exceeds i64".to_string()))?;
    let path = lease_path(subject_id);
    let Some(current) = store.read_text_versioned(&path).await? else {
        return Ok(None);
    };
    let mut lease: PlacementLease = serde_json::from_str(&current.content)?;
    if lease.schema_version != SCHEMA_VERSION {
        return Err(StorageError::Other(format!(
            "unsupported placement lease schema_version {}",
            lease.schema_version
        )));
    }
    if lease.token != token {
        return Ok(None);
    }
    lease.expires_at = (now + Duration::seconds(ttl_seconds)).to_rfc3339();
    let content = serde_json::to_string(&lease)?;
    match store
        .compare_and_swap_text(&path, &current.version, &content)
        .await
    {
        Ok(_) => Ok(Some(lease)),
        Err(StorageError::StorageConflict(_)) => Ok(None),
        Err(error) => Err(error),
    }
}

pub async fn release_placement_lease(
    store: &JobStorage,
    subject_id: &str,
    token: &str,
) -> Result<bool, StorageError> {
    let path = lease_path(subject_id);
    let Some(current) = store.read_text_versioned(&path).await? else {
        return Ok(false);
    };
    let mut lease: PlacementLease = serde_json::from_str(&current.content)?;
    if lease.schema_version != SCHEMA_VERSION {
        return Err(StorageError::Other(format!(
            "unsupported placement lease schema_version {}",
            lease.schema_version
        )));
    }
    if lease.token != token {
        return Ok(false);
    }
    lease.expires_at = Utc::now().to_rfc3339();
    let content = serde_json::to_string(&lease)?;
    match store
        .compare_and_swap_text(&path, &current.version, &content)
        .await
    {
        Ok(_) => Ok(true),
        Err(StorageError::StorageConflict(_)) => Ok(false),
        Err(error) => Err(error),
    }
}

/// Relinquish `owned` to the exact state already recorded by an
/// interruption-safe caller. Returning `false` means the object is absent or
/// a different token owns it; neither case is overwritten. Seeing `released`
/// verbatim is an adopted success, so replay never extends a relinquished
/// token and never invents a second release timestamp.
pub async fn release_placement_lease_exact(
    store: &JobStorage,
    owned: &PlacementLease,
    released: &PlacementLease,
) -> Result<bool, StorageError> {
    if released.schema_version != owned.schema_version
        || released.subject_id != owned.subject_id
        || released.decision_id != owned.decision_id
        || released.token != owned.token
        || released.holder != owned.holder
        || released.acquired_at != owned.acquired_at
    {
        return Err(StorageError::Other(
            "exact lease release changed fields other than expires_at".to_string(),
        ));
    }
    let path = lease_path(&owned.subject_id);
    let Some(current) = store.read_text_versioned(&path).await? else {
        return Ok(false);
    };
    let current_lease: PlacementLease = serde_json::from_str(&current.content)?;
    if current_lease == *released {
        return Ok(true);
    }
    if current_lease.schema_version != SCHEMA_VERSION {
        return Err(StorageError::Other(format!(
            "unsupported placement lease schema_version {}",
            current_lease.schema_version
        )));
    }
    if current_lease.token != owned.token {
        return Ok(false);
    }
    store
        .compare_and_swap_text(&path, &current.version, &serde_json::to_string(released)?)
        .await?;
    Ok(true)
}

fn lease_path(subject_id: &str) -> String {
    let key = hex::encode(Sha256::digest(subject_id.as_bytes()));
    format!("{LEASE_PREFIX}/{key}.json")
}
