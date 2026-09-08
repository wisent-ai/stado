//! The two Apple login verbs: capture a challenge, and issue the grants for one.

mod issue;
mod relay;

// `relay_apple_challenge` names `super::service::host_sudo_password` on the same path
// it used while this module was one file, so the import keeps that path resolving.
use crate::cli::service;

pub use issue::issue_apple_capabilities;
pub use relay::relay_apple_challenge;
