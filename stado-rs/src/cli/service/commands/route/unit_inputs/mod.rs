//! Where the verbs that change a running unit's inputs land: one routing
//! file per declaration block, at the path its declaration sits at.

pub(in crate::cli::service::commands) mod credentials;
pub(in crate::cli::service::commands) mod environment;
