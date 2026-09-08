//! Runtime placement operations over a registry document.

mod contract;
mod transaction;

pub use contract::validate_registry_contract;
pub use transaction::{claim_transaction, profile_for_services, release_transaction};
