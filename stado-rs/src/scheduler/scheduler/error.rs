//! The scheduler-layer error type.
//!
//! Quota, storage and provider failures plus the agent startup-template
//! contract violations that must abort a tick *before* the provider API is
//! called, so a template or config gap never costs a billable VM.

use crate::providers::ProviderError;
use crate::queue::StorageError;
use crate::scheduler::quota::QuotaError;

/// Scheduler-layer error.
#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    /// Quota read failures (live cloud quotas + overlay).
    #[error(transparent)]
    Quota(#[from] QuotaError),
    /// Queue storage failures.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// Provider create/list failures.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// A `${KEY}` the bundled startup templates reference but that no
    /// producer filled. Left unsubstituted the placeholder reaches the VM
    /// verbatim and `set -u` aborts the boot before the agent starts, so
    /// dispatch refuses to create the instance rather than pay for a VM
    /// that can never claim a job.
    #[error(
        "startup-script placeholder ${{{key}}} was never substituted; supply it from \
         scheduler::dispatch::agent::deployment_substitutions or the coordinator secrets"
    )]
    UnresolvedPlaceholder {
        /// Placeholder name only, never its value — secrets stay unlogged.
        key: String,
    },
    /// A template omitted an export owned by the dispatcher. Checking the
    /// source contract before substitution prevents an apparently successful
    /// render from silently dropping deployment state.
    #[error("agent startup template for {provider} omits required ${{{key}}} export")]
    MissingStartupExport { provider: String, key: String },
    /// A required immutable boot coordinate is absent. This must fail before
    /// the provider API is called, not after a billable machine starts.
    #[error(
        "agent startup setting {key} is empty; configure {env} (config key {config_key}) \
         with the immutable runtime artifact before dispatch"
    )]
    MissingStartupSetting {
        key: String,
        env: &'static str,
        config_key: &'static str,
    },
    #[error(
        "agent startup setting {key} is invalid: {reason}; fix {env} \
         (config key {config_key}) before dispatch"
    )]
    InvalidStartupSetting {
        key: String,
        env: &'static str,
        config_key: &'static str,
        reason: &'static str,
    },
}
