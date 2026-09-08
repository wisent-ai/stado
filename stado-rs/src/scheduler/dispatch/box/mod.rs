//! Fenced dispatch and exhaustive reconciliation for Box-backed jobs.
//!
//! Port of `stado/scheduler/dispatch/box/__init__.py`.
//!
//! (Python also defines an unused `_TERMINAL_BOX_STATES` frozenset — the
//! terminal checks are written inline at each use site, as ported here.)

pub mod output;
mod passes;
pub mod runtime;

/// Named out of tree by `crate::providers::r#box`, whose
/// `box_dispatch_to_provider_error` lifts this error into `ProviderError`, and
/// by the `output` and `runtime` siblings as `super::BoxDispatchError`; it is
/// also the error every function re-exported below carries.
pub use passes::BoxDispatchError;
/// `cancel_box_for_legacy_move` is named by `crate::providers::r#box`, whose
/// `delete_instance` bridges through the fenced cancel path when a running/
/// job still references the box; `cancel_box_job` is the plain cancel arm
/// beside it, keeping its published `dispatch::box::` path.
pub use passes::{cancel_box_for_legacy_move, cancel_box_job};
/// `run_box_tick` is named by `crate::coordinator::passes::tick` and
/// doc-linked from `crate::coordinator::passes::providers`;
/// `dispatch_box_jobs` and `reconcile_box_jobs` are the two halves it
/// composes, each keeping its published `dispatch::box::` path.
pub use passes::{dispatch_box_jobs, reconcile_box_jobs, run_box_tick};
