//! The command surface: every verb, its arguments, and the one match that
//! routes a parsed verb to the function answering it.

pub(in crate::cli::secrets) mod dispatch;
pub(in crate::cli::secrets) mod subcommands;
pub(in crate::cli::secrets) mod surface;

// `dispatch` reaches the host verbs and the seed-freshness judge through
// `super::`, the same two paths it named when this surface was one file
// directly under `crate::cli`. Re-exported here so those paths still resolve
// and the dispatch body reads exactly as it did.
pub(in crate::cli::secrets) use crate::cli::{host, seed_freshness};
