//! What the release channel holds: whether one object is there, how large
//! it is, which source revision claimed it, and every coordinate published.

pub(in crate::cli::storage) mod claim;
pub(in crate::cli::storage) mod coordinates;
pub(in crate::cli::storage) mod present;
