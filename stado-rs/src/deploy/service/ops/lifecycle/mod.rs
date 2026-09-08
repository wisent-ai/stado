//! Start, restart, reload, stop, retire and probe — including the one system
//! LaunchDaemon repair that needs no privilege and the privileged path behind
//! it.

mod daemon;
mod restart;
mod stop;

// `privileged` exposes only the sudo path `restart` calls, which reaches it
// directly as `super::privileged`; it re-exports nothing.
mod privileged;

pub use daemon::*;
pub use restart::*;
pub use stop::*;
