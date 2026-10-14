//! The reclamation itself: what happens once a revision has been selected.
//!
//! [`barrier`] builds, discards and recovers the private hard-linked copy of
//! the `.locks` namespace; [`exchange`] performs the atomic swap that puts
//! that copy in place for the duration of one deletion; [`recheck`] re-proves
//! the selected revision against the disk; [`unlink`] removes it, refs first,
//! then the snapshot tree deepest-first, then the exclusive blobs.

pub(in crate::providers::local::disk_cleanup::hf) mod barrier;
pub(in crate::providers::local::disk_cleanup::hf) mod exchange;
pub(in crate::providers::local::disk_cleanup::hf) mod recheck;
pub(in crate::providers::local::disk_cleanup::hf) mod unlink;
