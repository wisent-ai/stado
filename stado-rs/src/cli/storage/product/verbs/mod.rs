//! The commands over one product object: read it, list a namespace, discard
//! an interrupted upload, remove it, and print its gateway URL.

pub(in crate::cli::storage) mod abort_upload;
pub(in crate::cli::storage) mod get;
pub(in crate::cli::storage) mod rm;
