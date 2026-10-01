//! The single-writer lock an operation is mutated under: taken by
//! compare-and-swap, checked between mutations, and released by the phase
//! that took it. A held lock is never stolen; a released one may be taken.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::cli::resources::journal::clock::now;
use crate::cli::resources::journal::names::{remote_path, validate_operation_id};
use crate::cli::resources::model::canonical_json_bytes;
use crate::cli::CmdError;

use super::{map_conflict, Journal};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OperationLease {
    owner: String,
    acquired_at: String,
    #[serde(default)]
    released_at: Option<String>,
}

impl Journal {
    pub async fn acquire(&self, operation_id: &str) -> Result<String, CmdError> {
        validate_operation_id(operation_id)?;
        let owner = format!(
            "{}-{}",
            crate::watchdog::hostname(),
            Uuid::new_v4().simple()
        );
        let lease = OperationLease {
            owner: owner.clone(),
            acquired_at: now(),
            released_at: None,
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
        if current.released_at.is_none() {
            return Err(CmdError::click(format!(
                "operation is locked by {} since {}; it stays locked until that run releases it",
                current.owner, current.acquired_at
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
        let lease: OperationLease = serde_json::from_str(&versioned.content)?;
        if lease.owner != owner || lease.released_at.is_some() {
            return Err(CmdError::click(
                "operation lock was lost before the next mutation",
            ));
        }
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
        lease.released_at = Some(now());
        let body = String::from_utf8(canonical_json_bytes(&lease)?)
            .map_err(|error| CmdError::click(error.to_string()))?;
        self.store
            .compare_and_swap_text(&path, &versioned.version, &body)
            .await
            .map_err(map_conflict)?;
        Ok(())
    }
}
