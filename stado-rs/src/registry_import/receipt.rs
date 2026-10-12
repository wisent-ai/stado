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

/// How an import ended. The receipt's `state` is exactly one of these, so a
/// reader matches the type instead of comparing words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportState {
    /// Accepted and persisted as a new generation.
    Imported,
    /// Accepted; every declaration was already present unchanged.
    Unchanged,
    /// Refused: existing registry state differs.
    Conflict,
    /// Refused: the source is not a valid registry document.
    Rejected,
}

impl ImportState {
    /// The word the receipt carries for this state.
    pub fn word(self) -> &'static str {
        match self {
            Self::Imported => "imported",
            Self::Unchanged => "unchanged",
            Self::Conflict => "conflict",
            Self::Rejected => "rejected",
        }
    }
}

impl std::fmt::Display for ImportState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.word())
    }
}

/// The complete, bounded answer returned by both public surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryImportReceipt {
    pub schema: String,
    pub state: ImportState,
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
    pub(super) fn empty(source_sha256: String, state: ImportState) -> Self {
        Self {
            schema: RECEIPT_SCHEMA.to_string(),
            state,
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
        matches!(self.state, ImportState::Imported | ImportState::Unchanged)
    }

    pub fn changed(&self) -> bool {
        self.state == ImportState::Imported
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
