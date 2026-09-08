//! The passes of one Box tick, split out of the module entry: the bounds and
//! layer error they share, the admit half, the reconcile half, and the entry
//! points that mint their own fence owner.
//!
//! `support` holds what more than one pass reads, so the constants and the
//! error type live beside the passes rather than above them.

mod admit;
mod reconcile;
mod session;
mod support;

/// The admit half of the tick, kept nameable at its published
/// `dispatch::box::` path and called by the sibling `session` module, whose
/// `run_box_tick` composes it with the reconcile half.
pub use admit::dispatch_box_jobs;
/// The reconcile half of the tick, kept nameable at its published
/// `dispatch::box::` path and called by the sibling `session` module.
pub use reconcile::reconcile_box_jobs;
/// `cancel_box_for_legacy_move` is named by `crate::providers::r#box` and
/// `run_box_tick` by `crate::coordinator::passes::tick`; `cancel_box_job` is
/// the plain cancel arm beside them at its published `dispatch::box::` path.
pub use session::{cancel_box_for_legacy_move, cancel_box_job, run_box_tick};
/// The error every function re-exported above carries, named out of tree by
/// `crate::providers::r#box` and by the `output` and `runtime` siblings one
/// level up.
pub use support::BoxDispatchError;
