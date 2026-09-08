//! The single-writer lock an operation is mutated under: an expiring lease
//! taken by compare-and-swap, renewed between mutations, and released by the
//! phase that took it. An expired lease may be stolen; a live one may not.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::cli::resources::journal::clock::{lease_duration, now, timestamp};
use crate::cli::resources::journal::names::{remote_path, validate_operation_id};
use crate::cli::resources::model::{canonical_json_bytes, SCHEMA_VERSION};
use crate::cli::CmdError;

use super::{map_conflict, Journal};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OperationLease {
    schema_version: u8,
    owner: String,
    acquired_at: String,
    expires_at: String,
}

impl Journal {
    pub async fn acquire(&self, operation_id: &str) -> Result<String, CmdError> {
        validate_operation_id(operation_id)?;
        let owner = format!(
            "{}-{}",
            crate::watchdog::hostname(),
            Uuid::new_v4().simple()
        );
        let now_value = Utc::now();
        let lease = OperationLease {
            schema_version: SCHEMA_VERSION,
            owner: owner.clone(),
            acquired_at: timestamp(now_value),
            expires_at: timestamp(now_value + lease_duration()),
        };
        let body = String::from_utf8(canonical_json_bytes(&lease)?)
            .map_err(|error| CmdError::click(error.to_string()))?;
        let path = remote_path(operation_id, "lock.json");
        if self.store.create_text_if_absent(&path, &body).await? {
            return Ok(owner);
        }
        let versioned = self
            .store
            .read_text_versioned(&path)
            .await?
            .ok_or_else(|| CmdError::click("operation lock disappeared"))?;
        let current: OperationLease = serde_json::from_str(&versioned.content)?;
        let expires = DateTime::parse_from_rfc3339(&current.expires_at)
            .map_err(|error| CmdError::click(format!("invalid operation lock: {error}")))?;
        if expires.with_timezone(&Utc) > Utc::now() {
            return Err(CmdError::click(format!(
                "operation is locked by {} until {}",
                current.owner, current.expires_at
            )));
        }
        self.store
            .compare_and_swap_text(&path, &versioned.version, &body)
            .await
            .map_err(map_conflict)?;
        Ok(owner)
    }

    pub async fn renew(&self, operation_id: &str, owner: &str) -> Result<(), CmdError> {
        let path = remote_path(operation_id, "lock.json");
        let versioned = self
            .store
            .read_text_versioned(&path)
            .await?
            .ok_or_else(|| CmdError::click("operation lock disappeared"))?;
        let mut lease: OperationLease = serde_json::from_str(&versioned.content)?;
        if lease.owner != owner {
            return Err(CmdError::click(
                "operation lock was lost before the next mutation",
            ));
        }
        lease.expires_at = timestamp(Utc::now() + lease_duration());
        let body = String::from_utf8(canonical_json_bytes(&lease)?)
            .map_err(|error| CmdError::click(error.to_string()))?;
        self.store
            .compare_and_swap_text(&path, &versioned.version, &body)
            .await
            .map_err(map_conflict)?;
        Ok(())
    }

    pub async fn release(&self, operation_id: &str, owner: &str) -> Result<(), CmdError> {
        let path = remote_path(operation_id, "lock.json");
        let Some(versioned) = self.store.read_text_versioned(&path).await? else {
            return Ok(());
        };
        let mut lease: OperationLease = serde_json::from_str(&versioned.content)?;
        if lease.owner != owner {
            return Err(CmdError::click("operation lock ownership changed"));
        }
        lease.expires_at = now();
        let body = String::from_utf8(canonical_json_bytes(&lease)?)
            .map_err(|error| CmdError::click(error.to_string()))?;
        self.store
            .compare_and_swap_text(&path, &versioned.version, &body)
            .await
            .map_err(map_conflict)?;
        Ok(())
    }
}
