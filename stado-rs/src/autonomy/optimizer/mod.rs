//! Global, atomic workload placement across local capacity and cloud providers.
//!
//! The components are the seams this file already carried: [`types`] holds
//! the records one pass carries, [`offers`] reads every unit of capacity the
//! policy allows placing on, [`candidates`] prices each offer for a job and
//! collects the reasons it cannot be used, and [`plan`] walks the queue,
//! leases the cheapest eligible target and writes the decisions. The name a
//! caller outside this module uses is re-exported here, so
//! `crate::autonomy::optimizer::plan_queued` resolves exactly as before.

mod candidates;
mod offers;
mod plan;
mod types;

// `super::policy` and `super::storage` for the moved lines that name
// `super::policy::<item>` verbatim in `plan` and `super::storage::<item>`
// verbatim in `plan` and `types`.
use crate::autonomy::policy;
use crate::autonomy::storage;

pub use plan::plan_queued;
pub use types::PlacementRunSummary;
