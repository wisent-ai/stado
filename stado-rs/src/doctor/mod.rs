//! Deployment preflight probes behind `stado doctor`.
//!
//! NO Python original: the Python CLI has no preflight. Every one of the
//! six blockers in the 2026-07-26 GCP-billing outage surfaced as a crash
//! loop or a silently empty UI instead of a check — an azure backend with
//! no storage account, an all-zero quota, an unreachable release channel,
//! a missing VM managed identity, a startup template that aborted on
//! `set -u`, and a fleet that was simply paused. Each of those is cheap to
//! interrogate directly; none of them was interrogated anywhere.
//!
//! Two properties are load-bearing:
//!
//! - **Fault isolation.** One probe failing must never suppress the rest,
//!   because the useful output is the WHOLE list — the outage looked like
//!   "quota is zero" until the release channel and the VM identity turned
//!   out to be broken too. Every probe captures its own error into its own
//!   [`Check`], exactly as each section of
//!   [`crate::monitor::billing::collect_billing`] captures its own. Probes
//!   additionally run under a shared deadline ([`PROBE_TIMEOUT`]) so a
//!   black-holed endpoint degrades to one FAIL row instead of hanging the
//!   command.
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

use std::time::Duration;

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

/// Ceiling on ONE probe. Bounds the command against a black-holed endpoint
/// — the failure mode of an unreachable release channel or a firewalled
/// cloud API, which drop packets rather than refusing them, so the socket
/// never returns. Derived digit-free from `u8::BITS`, the same way
/// `crate::cli::default_mail_results` derives its page size. Probes run
/// concurrently, so this bounds the whole command and not one row of it.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(u8::BITS as u64);
