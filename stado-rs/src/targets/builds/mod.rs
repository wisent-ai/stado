//! Which build a target is carrying, and how far it has drifted from the one
//! the fleet expects.
//!
//! The registry's `builds` recipes and the `deliveries`/`passes` batching
//! written on top of them used to live here. Nobody had asked for either:
//! they spent the fleet's build budget on single commits, and they were
//! removed. What a host carries, below, is all this module was ever asked
//! for.

mod build_skew;
mod routing;

pub use build_skew::*;
pub use routing::{platform_accepts_job, platform_job_os_arch};
