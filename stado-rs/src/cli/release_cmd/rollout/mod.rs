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
    target: String,
    #[arg(long)]
    product: Option<String>,
    #[arg(long)]
    once: bool,
    #[arg(long, default_value_t = 15)]
    interval_seconds: u64,
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub struct ReleaseStatusArgs {
    pub product: Option<String>,
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
