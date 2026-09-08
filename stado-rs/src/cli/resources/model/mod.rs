//! Versioned, provider-neutral resource operation model.
//!
//! `kinds` holds the closed vocabularies, `plan` the shape an operation takes
//! on the wire, and `validate` everything a plan has to satisfy before anyone
//! acts on it. The canonical byte encoding every digest is taken over lives
//! here, beside the re-exports.

mod kinds;
mod plan;
mod validate;

use serde::Serialize;

use super::super::CmdError;

pub use kinds::{
    ActionKind, Authorization, FindingDisposition, Intent, ProviderKind, Reversibility,
    SCHEMA_VERSION,
};
pub use plan::{
    Action, Condition, Finding, InventorySnapshot, OperationScope, Plan, ResourceLocator, Rollback,
    SourceSnapshot,
};

pub fn canonical_json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, CmdError> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}
