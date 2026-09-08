//! Credentials: the provider identities dispatch runs behind, the vault this
//! machine writes through, the broker the queue agent reads secrets from, and
//! the read contract that broker enforces.

pub(in crate::doctor) mod agent;
pub(in crate::doctor) mod contract;
pub(in crate::doctor) mod providers;
pub(in crate::doctor) mod vault;
