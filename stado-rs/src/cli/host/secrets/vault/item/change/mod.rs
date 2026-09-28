//! Changing an item the vault already holds: delete it, give it a new id, or
//! replace its tags.

pub(in crate::cli::host) mod delete;
pub(in crate::cli::host) mod rename;
pub(in crate::cli::host) mod retag;
