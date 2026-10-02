//! Timestamped observations, separate from declarations of desired state.
//!
//! A valid declaration does not prove that its consumer can reach the declared
//! service. An observation names the fact, the checking host (`vantage`), the
//! outcome (`state`), its diagnostic detail, and its UTC timestamp (`at`).
//! Writers and readers share the same fact names and outcome constants.
//!
//! Reachability distinguishes an answer (`observed`), a failed probe
//! (`unreachable`), and a probe that could not run (`unverified`). Ownership
//! checks can additionally report `misowned` or `standby_serving`. Unknown
//! outcomes from newer writers remain intact rather than becoming successes.
//!
//! Freshness uses the caller's TTL. `Fresh` carries a recent observation;
//! `Stale` retains history without treating it as current evidence. `Never`
//! means there is no observation, not that a service passed or failed. A row
//! with an unreadable timestamp is stale. Shared display paths use
//! [`DEFAULT_TTL`], currently one hour.
//!
//! Storage is `~/.stado/observations.json`, owner-only and replaced by a
//! same-directory rename, so readers do not see a partially written record.

mod display;
mod observation;
mod staleness;
mod store;

pub use display::{describe, describe_in, render};
pub use observation::{
    service_fact, Observation, MISOWNED, OBSERVED, STANDBY_SERVING, UNREACHABLE, UNVERIFIED,
};
pub use staleness::{freshness, freshness_in, Freshness, DEFAULT_TTL};
pub use store::{load, record};
