//! Files on one host: delivery, retirement, removal, and the local
//! storage replica.

pub(in crate::cli::host) mod forwarding;
pub(in crate::cli::host) mod remove;
pub(in crate::cli::host) mod retire;
pub(in crate::cli::host) mod storage;
