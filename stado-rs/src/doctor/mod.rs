//! Deployment preflight probes behind `stado doctor`.
//!
//! Probe deployment dependencies directly so configuration, quota, identity
//! and reachability failures appear as separate findings rather than an
//! empty interface or a failing process.
//!
//! Two properties are load-bearing:
//!
//! - **Fault isolation.** One probe failing must not suppress the others.
//!   Each captures its own error in its [`Check`], like each section of
//!   [`crate::monitor::billing::collect_billing`]. Concurrent probes keep
//!   unrelated dependencies independent.
//! - **Same code path as production.** The template probe renders through
//!   [`crate::scheduler::dispatch::agent::bundled_template_for`] with
//!   credentials resolved from Skarbiec and
//!   [`crate::scheduler::dispatch::agent::deployment_substitutions`] — the
//!   dispatcher's own text, secrets and config. A preflight that rendered
//!   its own copy would prove nothing about what dispatch ships.
//!
//! Read-only except for [`check_storage_round_trip`], which is the only
//! answer to "is the queue empty or is the store unreachable" and therefore
//! has to actually write. It writes, reads back and deletes one
//! self-describing object under [`PROBE_PREFIX`], and the delete runs even
//! when the read-back fails — a doctor that litters the queue store on
//! every bad run is worse than no doctor. [`PROBE_PREFIX`] is deliberately
//! outside `queue::copy::CANONICAL_PREFIXES`, for the same reason
//! `queue::copy::SENTINEL_PATH` is: a diagnostic probe is precisely what a
//! backend migration must not carry across.

use crate::config;

mod fleet;
mod plane;
mod report;
mod runner;

pub use self::plane::object_auth::object_auth_verdict;
pub use self::report::{Check, Report, RunScope, Status};
pub use self::runner::run;

pub(in crate::doctor) use self::report::Findings;

/// Prefix the storage round-trip probe writes under. Not a queue-state
/// prefix and deliberately absent from `queue::copy::CANONICAL_PREFIXES`,
/// so a cutover copy never carries a diagnostic object to the new store.
pub const PROBE_PREFIX: &str = "diagnostics/";

/// Provider name of the device-local deployment, which has no cloud API to
/// authenticate against and no agent VMs to dispatch.
/// `crate::coordinator::resolve_providers` skips it for the same reason.
const LOCAL_PROVIDER: &str = crate::capabilities::ProviderId::Local.as_str();

fn provider_enabled(provider: crate::capabilities::ProviderId) -> bool {
    config::wc_providers()
        .iter()
        .any(|name| provider.matches(name))
}

fn storage_adapter(name: &str) -> Option<crate::capabilities::StorageAdapter> {
    crate::capabilities::storage_adapter(name)
}
