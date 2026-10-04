//! Rollout state in the registry: the reviewed policy, the channel and
//! generation arithmetic of a promotion, and the reads that report what the
//! fleet desires against what it runs.

use std::path::PathBuf;

use clap::Args;
use serde::Deserialize;

use crate::release_control::{ProductReleasePolicy, ReleaseChannel};

pub(super) mod policy;
pub(super) mod promote;
pub(super) mod reconcile;
pub(super) mod status;

#[derive(Args)]
pub struct ReleasePolicyApplyArgs {
    /// JSON document containing exactly `product` and `policy`.
    #[arg(long)]
    file: PathBuf,
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub struct ReleasePolicyTargetRemoveArgs {
    /// Product whose rollout policy loses the host.
    pub product: String,
    /// Registry target the product is no longer released to.
    #[arg(long)]
    pub target: String,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct ReleasePolicyRemoveArgs {
    /// Product no longer rolled out by release control.
    pub product: String,
    #[arg(long)]
    pub json: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleasePolicyDocument {
    product: String,
    policy: ProductReleasePolicy,
}

#[derive(Args)]
pub struct ReleasePromoteArgs {
    pub product: String,
    pub version: String,
    #[arg(long, value_enum, default_value_t = ChannelArg::Stable)]
    channel: ChannelArg,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum ChannelArg {
    Candidate,
    Stable,
}

impl From<ChannelArg> for ReleaseChannel {
    fn from(channel: ChannelArg) -> Self {
        match channel {
            ChannelArg::Candidate => Self::Candidate,
            ChannelArg::Stable => Self::Stable,
        }
    }
}

#[derive(Args)]
pub struct ReleaseAgentArgs {
    #[arg(long)]
    pub(crate) target: String,
    #[arg(long)]
    pub(crate) product: Option<String>,
    #[arg(long)]
    pub(crate) once: bool,
    /// Seconds between reconcile passes when the agent runs as a loop.
    /// Required without --once.
    #[arg(long)]
    pub(crate) interval_seconds: Option<u64>,
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Args)]
pub struct ReleaseStatusArgs {
    pub product: Option<String>,
    /// One pipeline run, by its id or the first characters of it as this
    /// command prints them; the target rows are left out, because the
    /// question is about the run. Found however far back it is.
    #[arg(long)]
    run: Option<String>,
    /// Only runs that published this version; with --run, both must hold.
    #[arg(long)]
    version: Option<String>,
    /// List only the newest N release runs; without it every recorded run is
    /// listed. Zero is refused: omit the flag to list every run.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    limit: Option<u64>,
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub struct ReleaseActiveBinaryArgs {
    pub product: String,
    /// Resolve for this registry target. Defaults to the current machine.
    #[arg(long)]
    target: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub struct ReleaseRollbackArgs {
    pub product: String,
    #[arg(long)]
    json: bool,
}
