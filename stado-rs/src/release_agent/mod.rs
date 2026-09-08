//! Host-side blue-green release reconciliation.
//!
//! The agent consumes exact desired release coordinates from the canonical
//! registry, verifies signed immutable artifacts, stages them in versioned
//! directories, starts a candidate on an internal port, and atomically changes
//! a stable loopback proxy. The previous process stays warm for the rollback
//! window. Failed digests are quarantined and cannot loop indefinitely.
//!
//! One phase per module: [`state`] is the journal this host keeps, [`rollout`]
//! is the candidate and the bind it is routed onto, and [`tick`] is one pass
//! and the loop that repeats it. Everything the rest of the crate reads is
//! re-exported here, so `crate::release_agent::<name>` stays the one spelling
//! of every name this agent publishes.

// Crate-visible, not private: the agent's own crate-internal names —
// `StateLock`, `ActiveBinary`, `ReleaseProcess` — are `pub(crate)` items that
// `pub(crate)` functions here carry in their signatures, and a phase module
// private to this one would make those types less reachable than the functions
// that return them.
pub(crate) mod rollout;
pub(crate) mod state;
#[cfg(test)]
mod tests;
pub(crate) mod tick;

pub(crate) use rollout::candidate::binary::active_binary;
pub(crate) use rollout::candidate::fetch::fetch_candidate;
pub use rollout::recover::run::{cause_run, CauseRun};
pub use rollout::recover::wall::{CauseHold, HoldGround};
pub use rollout::serving::proxy::proxy;
pub(crate) use state::document::{acquire_state_lock, atomic_json};
pub use state::document::{host_state_path, parse_state_document, state_document_bytes};
pub use state::evidence::host_log_path;
pub use state::records::{HostReleaseState, ProcessRecord, QuarantineRecord, RolloutPhase};
pub use state::status::{publish_service_release_status, release_status_uri};
pub use tick::once::{agent, reconcile_once};
