//! Changing an item the vault already holds: delete it, give it a new id,
//! replace its tags, or return it to the owner's control.

pub(in crate::cli::host) mod delete;
pub(in crate::cli::host) mod reclaim;
pub(in crate::cli::host) mod rename;
pub(in crate::cli::host) mod retag;
