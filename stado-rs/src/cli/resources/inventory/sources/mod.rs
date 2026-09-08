//! One file per fault-isolated source.
//!
//! Every inspector answers with a `SourceReport` and never with an error, so a
//! cloud, credential or queue failure degrades exactly one source instead of
//! erasing the resources the others returned.

mod billing;
mod compute;
mod registries;
mod storage;

pub(super) use billing::inspect_billing;
pub(super) use compute::inspect_compute;
pub(super) use registries::{inspect_gcp, inspect_registry};
pub(super) use storage::inspect_storage;
