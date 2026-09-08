//! The commands that address two stores, or a directory and an archive:
//! the copier, its disaster-recovery form, the read-only comparison, and the
//! deterministic release archive.

pub(in crate::cli::storage) mod archive;
pub(in crate::cli::storage) mod copy;
pub(in crate::cli::storage) mod verify;
