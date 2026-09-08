//! The running-jobs pass: one walk over running/ deciding each job's exit
//! condition.
//!
//! [`pass`] holds the walk itself — the COMPLETED/FAILED finalizers and the
//! staleness guards that select a requeue — and [`vm_delete`] the ghost-VM
//! kill it issues once a requeue has actually won.

mod pass;
mod vm_delete;

pub use pass::check_running_jobs;
