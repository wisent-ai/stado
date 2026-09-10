//! What a pass DOES: resolving the policy in force, and the three repairs a
//! declaration may permit.
//!
//! Everything here touches the host — launchd and systemd domains, the
//! process table, and the recovery programs this release ships — which is
//! why it is separated from [`super::declaration`], where nothing runs.

pub mod pass;
pub mod policy;
pub mod repairs;
pub mod session;
