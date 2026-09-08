//! Additive, idempotent adoption of an existing Stado registry-v2 document.
//!
//! The importer is the one operation used by the CLI and dashboard API. It
//! validates the complete source before opening the destination, merges named
//! fleet records without replacing anything already declared, validates the
//! complete candidate, and commits it with compare-and-swap. A replay either
//! reports every source record as unchanged or imports only records that are
//! still absent.

mod commit;
mod merge;
mod receipt;
mod source;

pub use commit::import_bytes;
pub use receipt::{RegistryImportConflict, RegistryImportError, RegistryImportReceipt};
