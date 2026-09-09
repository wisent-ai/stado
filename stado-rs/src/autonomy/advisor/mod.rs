//! Rightsizing, scheduling, storage-lifecycle, and commitment recommendations.
//!
//! The components are the seams this file already carried: `summary` holds the
//! counters one advisory pass returns, `run` walks the inventory and publishes
//! one recommendation per finding, `decision` builds and identifies the
//! decision record each finding becomes, and `signals` answers the questions
//! the walk asks about a resource -- whether it is underutilized, how it is
//! utilized, whether its storage aged out, and which of its dependencies
//! cross a provider or region boundary. Every name a caller outside this
//! module uses is re-exported here, so `crate::autonomy::advisor::<item>`
//! resolves exactly as before.

mod decision;
mod run;
mod signals;
mod summary;

// `super::storage` for the moved line that names
// `super::storage::write_decision` verbatim in `run`.
use crate::autonomy::storage;

pub use run::publish_recommendations;
pub use summary::AdvisorSummary;
