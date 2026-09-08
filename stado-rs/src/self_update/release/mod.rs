//! The release read: everything the update knows before a single byte is
//! fetched — the failure vocabulary, the published binary set and host
//! triple, and the fetcher bound to one exact configured coordinate.

pub(super) mod binaries;
pub(super) mod error;
pub(super) mod fetcher;
