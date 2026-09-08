//! The passes one coordinator tick runs, in the order it runs them.
//!
//! - `providers` resolves `WC_PROVIDERS` into the tick's arms.
//! - `autonomy` runs the inventory, placement, reconciliation, advice, cost
//!   and lifecycle stage over those arms.
//! - `tick` composes every pass, plus the per-arm check/reap/schedule work,
//!   into the single `run_tick` entry both deployment shapes call.

mod autonomy;
mod providers;
mod tick;

pub(crate) use autonomy::run_autonomy_once;

pub use providers::{resolve_providers, ResolvedProvider};
pub use tick::{run_tick, CoordinatorError};
