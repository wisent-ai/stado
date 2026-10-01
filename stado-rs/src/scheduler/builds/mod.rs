//! What the fleet may spend on compiling, and the charge that spends it.
//!
//! This module holds no poller: no registry build recipes, no `git ls-remote`
//! per enabled recipe on every coordinator tick, no job per new commit per
//! platform. A poller fills the queue with builds of single commits while
//! the rule is the opposite: work is written and pushed, and builds happen
//! separately and rarely.
//!
//! What remains is the ration and its enforcement, which the release pipeline
//! asks for and which nothing may bypass: [`BuildBudget`] is the day's
//! ceiling read from the registry, and [`charge`] takes from it when a
//! compiling job is submitted or claimed.

pub mod approval;
mod budget;
mod charge;

pub use budget::{BuildBudget, BUILD_BUDGET_KEY, DEFAULT_DAILY_BUILD_LIMIT};
pub use charge::{charge, compiles, BUILD_VERSION_FILE};
