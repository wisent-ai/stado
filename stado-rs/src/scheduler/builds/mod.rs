//! What the fleet may spend on compiling, and the charge that spends it.
//!
//! This module used to hold a poller: registry build recipes, `git ls-remote`
//! per enabled recipe on every coordinator tick, and one job per new commit
//! per platform. Nobody asked for it. It was added by sessions that had been
//! refused a test run and read "use a build recipe" in the refusal, and by
//! 2026-09-21 it had filled the queue with builds of single commits while the
//! operator's own rule says the opposite: work is written and pushed, and
//! builds happen separately and rarely. He removed it by name: "kto stworzyl
//! pollers. kto stworzyl recipe. kto prosil o ta funkcjonalnosc" — "to usun
//! ta funkcjonalnosc".
//!
//! What remains is the ration and its enforcement, which the release pipeline
//! asks for and which nothing may bypass: [`BuildBudget`] is the day's
//! ceiling read from the registry, and [`charge`] takes from it when a
//! compiling job is submitted or claimed.

mod budget;
mod charge;

pub use budget::{BuildBudget, BUILD_BUDGET_KEY, DEFAULT_DAILY_BUILD_LIMIT};
pub use charge::{charge, compiles, compiling, BUILD_VERSION_FILE};
