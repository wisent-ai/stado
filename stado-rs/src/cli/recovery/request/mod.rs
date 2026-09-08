//! The parsed `stado recovery migrate` request and the shapes it produces.
//!
//! Nothing here reaches the network or the filesystem. The types are the
//! command's argument surface plus the two records the later steps carry: the
//! config bytes prepared once and reused, and one service resolved down to the
//! unit file that proves where its storage routing comes from.
//!
//! [`validate`] refuses an impossible request before any store is fenced;
//! [`dry_run`] renders the same request as a plan.

pub(super) mod dry_run;
pub(super) mod validate;

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::str::FromStr;

use clap::Args;

use crate::cli::storage::EndpointArgs;
use crate::deploy::service;
use crate::queue::control;
use crate::queue::copy::DEFAULT_CONCURRENCY;
use crate::targets::ComputeTarget;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ServiceRef {
    pub(super) host: String,
    pub(super) service: String,
}

impl std::fmt::Display for ServiceRef {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}:{}", self.host, self.service)
    }
}

impl FromStr for ServiceRef {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let (host, service) = raw.split_once(':').ok_or_else(|| {
            format!("{raw:?} must be HOST:SERVICE (for example mac-mini:stado-agent)")
        })?;
        if host.is_empty()
            || service.is_empty()
            || host.chars().any(char::is_whitespace)
            || service.chars().any(char::is_whitespace)
        {
            return Err(format!("{raw:?} must contain a non-empty HOST and SERVICE"));
        }
        Ok(Self {
            host: host.to_string(),
            service: service.to_string(),
        })
    }
}

#[derive(Args, Debug)]
pub struct RecoveryMigrateArgs {
    #[command(flatten)]
    pub(super) ends: EndpointArgs,
    /// Every source writer Stado must stop before copying. HOST:SERVICE; repeatable. Omit only with --source-offline.
    #[arg(long = "writer")]
    pub(super) writers: Vec<ServiceRef>,
    /// Assert that no unlisted source writer can run, including schedulers, Cloud Functions, Cloud Run jobs, coordinators, monitors, and agents.
    #[arg(long)]
    source_offline: bool,
    /// Service to restart on the destination after config cutover. HOST:SERVICE; repeatable. Every activated service is fenced first.
    #[arg(long = "activate")]
    pub(super) activate: Vec<ServiceRef>,
    /// Complete compute-provider allowlist after cutover. Repeatable; gcp is rejected.
    #[arg(long = "enable-provider", required = true)]
    pub(super) enable_providers: Vec<String>,
    /// Resume dispatch and claims after every other step. Without it the destination stays paused.
    #[arg(long)]
    pub(super) resume: bool,
    /// Maximum seconds to wait for running/ to drain on each store.
    #[arg(long, default_value_t = control::default_drain_timeout_s())]
    pub(super) drain_timeout: u64,
    /// Objects copied in parallel.
    #[arg(long, default_value_t = default_concurrency())]
    pub(super) concurrency: NonZeroUsize,
    /// Config file to atomically rewrite. Defaults to Stado's resolved file.
    #[arg(long)]
    pub(super) config: Option<PathBuf>,
    /// Attach GCP billing only for source fencing, copy, and verification, then detach it.
    #[arg(long)]
    pub(super) manage_gcp_billing: bool,
    /// GCP project whose billing window may be managed.
    #[arg(long)]
    pub(super) gcp_project: Option<String>,
    /// Full billingAccounts/... name restored for the migration window.
    #[arg(long)]
    pub(super) gcp_billing_account: Option<String>,
    /// Must exactly repeat --gcp-project before a billable API call is made.
    #[arg(long)]
    confirm_billing_window: Option<String>,
    /// Validate and print the plan; perform no network or filesystem writes and no billing change.
    #[arg(long)]
    pub(super) dry_run: bool,
}

fn default_concurrency() -> NonZeroUsize {
    NonZeroUsize::new(DEFAULT_CONCURRENCY).expect("copy concurrency is non-zero")
}

pub(super) struct PreparedConfig {
    pub(super) path: PathBuf,
    pub(super) bytes: Vec<u8>,
}

#[derive(Clone)]
pub(super) struct ResolvedService {
    pub(super) reference: ServiceRef,
    pub(super) target: ComputeTarget,
    pub(super) service: service::ManagedService,
    pub(super) config_path: String,
    pub(super) activate: bool,
}
