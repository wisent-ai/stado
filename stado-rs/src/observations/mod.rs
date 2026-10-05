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
//! A reader gets the newest look at a fact (`Seen`) with its age, or `Never`
//! when nobody looked; `Never` means there is no observation, not that a
//! service passed or failed. No window decides when a look stops counting: a
//! look describes its own moment, and a judgement that needs the present asks
//! again or compares the look's content with what has changed since.
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
pub use staleness::{freshness, freshness_in, Freshness};
pub use store::{load, record};
