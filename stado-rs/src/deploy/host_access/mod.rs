//! Who and what may reach a host: the ssh key it trusts, the resolver key it
//! serves with, the forwards opened to it, and the accounts taken off it.

pub mod forward;
pub(crate) mod native;
pub mod resolver_key;
pub mod ssh_key;
pub mod user_delete;

/// The port the native SSH session dials on every host: the one port a
/// registry target is known to answer on, which is what a route carried
/// over that session can be proven against.
pub use native::SSH_PORT;
