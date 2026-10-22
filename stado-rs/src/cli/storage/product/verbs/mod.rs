//! The commands over one product object: read it, list a namespace, discard
//! an interrupted upload, remove it, print its gateway or presigned URL, and
//! describe the S3 endpoint a namespace is reached through.

pub(in crate::cli::storage) mod abort_upload;
pub(in crate::cli::storage) mod get;
pub(in crate::cli::storage) mod rm;
pub(in crate::cli::storage) mod s3;
