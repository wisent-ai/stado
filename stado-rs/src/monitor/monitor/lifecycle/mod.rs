//! The lifecycle moves both monitor passes reach for.
//!
//! Every move here re-reads the running document ([`current`]) and pins its
//! version, so a worker whose lease renewal lands first wins the race instead
//! of being requeued while it is still executing. The three verdicts differ
//! only in what they charge the move to: [`restart`] the restart budget,
//! [`preempt`] the Spot preempt counter, and [`orphan`] the dead local@ host
//! no VM reaper visits.

mod current;
mod orphan;
mod preempt;
mod restart;

pub(super) use orphan::requeue_dead_local_host_orphan;
pub(super) use preempt::requeue_preempted;
pub(super) use restart::{requeue, requeue_jids_after_reap};
