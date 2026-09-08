//! Typed resource executors. Plans cannot inject arbitrary REST methods or URLs.
//!
//! [`Context`] is the whole surface a caller drives: inspect, apply, restore
//! and wait. Behind it, `gcp` holds the authenticated REST client and one
//! typed method per resource family, `backup` edits the active config file,
//! and `conditions` decides whether observed state satisfies a plan.

mod backup;
mod conditions;
mod context;
mod gcp;

pub use conditions::{conditions_match, explain_mismatch};
pub use context::Context;
