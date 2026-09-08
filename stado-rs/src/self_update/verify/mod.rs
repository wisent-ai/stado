//! The download and its verification: which installed files are in scope,
//! the checksums every candidate is measured against, and the driver that
//! aborts before the first rename if any of it fails.

pub(super) mod install;
pub(super) mod sums;
pub(super) mod targets;
