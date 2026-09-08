//! The receipt both public surfaces return, and the operational errors that
//! are not receipts.

use serde::{Deserialize, Serialize};

use crate::queue::StorageError;

pub(super) const RECEIPT_SCHEMA: &str = "stado.registry-import-receipt.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryImportConflict {
    pub path: String,
    pub reason: String,
}

/// The complete, bounded answer returned by both public surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryImportReceipt {
    pub schema: String,
    pub state: String,
    pub source_sha256: String,
    pub generation: Option<String>,
    pub previous_generation: Option<String>,
    pub imported_targets: Vec<String>,
    pub unchanged_targets: Vec<String>,
    pub imported_fleets: Vec<String>,
    pub unchanged_fleets: Vec<String>,
    pub imported_sections: Vec<String>,
    pub unchanged_sections: Vec<String>,
    pub conflicts: Vec<RegistryImportConflict>,
    pub rejected: Vec<String>,
}

impl RegistryImportReceipt {
    pub(super) fn empty(source_sha256: String, state: &str) -> Self {
        Self {
            schema: RECEIPT_SCHEMA.to_string(),
            state: state.to_string(),
            source_sha256,
            generation: None,
            previous_generation: None,
            imported_targets: Vec::new(),
            unchanged_targets: Vec::new(),
            imported_fleets: Vec::new(),
            unchanged_fleets: Vec::new(),
            imported_sections: Vec::new(),
            unchanged_sections: Vec::new(),
            conflicts: Vec::new(),
            rejected: Vec::new(),
        }
    }

    pub fn accepted(&self) -> bool {
        matches!(self.state.as_str(), "imported" | "unchanged")
    }

    pub fn changed(&self) -> bool {
        self.state == "imported"
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryImportError {
    #[error("registry import could not open or persist the canonical registry: {0}")]
    Storage(String),
    #[error("canonical registry at generation {generation} is invalid: {reason}")]
    CanonicalInvalid { generation: String, reason: String },
    #[error("registry import verification returned different bytes or generation")]
    Verification,
}

impl From<StorageError> for RegistryImportError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error.to_string())
    }
}
