//! Which build a target is carrying, and how far it has drifted from the one
//! the fleet expects.
//!
//! The registry's `builds` recipes and the `deliveries`/`passes` batching
//! written on top of them lived here until 2026-09-21. Nobody had asked for
//! either: they were added by sessions that a refusal had pointed at a build
//! recipe, and they spent the fleet's day on single commits. The operator
//! removed them by name — "to usun ta funkcjonalnosc" — and what a host
//! carries, below, is all this module was ever asked for.

mod build_skew;
mod routing;

pub use build_skew::*;
pub use routing::{platform_accepts_job, platform_job_os_arch};
