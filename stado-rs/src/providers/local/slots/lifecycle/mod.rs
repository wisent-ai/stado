//! The transitions a slot makes: the claim and spawn, the cooperative yield
//! back to the queue, and the tick that carries a live slot to its durable
//! terminal state.
//!
//! `disk_cleanup` is imported here rather than in the parts because the
//! janitor's work-directory creation is named `super::disk_cleanup::...`
//! exactly as it was when this was one file.

use crate::providers::local::disk_cleanup;

use super::*;

mod advance;
mod expiry;
mod start;

pub use advance::*;
pub use expiry::*;
pub use start::*;
