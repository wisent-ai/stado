//! `stado release submit` and the pipeline it drives: source identity and
//! upload, platform build jobs, qualification, publication, promotion,
//! delivery, and the receipts and reports each stage leaves behind.
//!
//! One component tree per stage family: [`run`] for the source identity, the
//! durable run object and its reports, [`build`] for the platform build jobs
//! and the worker that executes them, [`publish`] for signing, publication
//! and promotion, and [`deliver`] for the deliveries and their redelivery.
//! Every name this module exposed before the split is re-exported here, so
//! `crate::cli::release_submit::NAME` still resolves.

use std::path::PathBuf;

use clap::Args;

use crate::release_pipeline::PipelineChannel;

// `builds`, not `build`: the repository's .gitignore excludes every directory
// named `build/`, so a component folder with that name is silently untracked.
mod builds;
pub mod changes;
mod deliver;
mod publish;
mod run;

pub use crate::cli::release_submit::builds::claimability::claimability;
pub use crate::cli::release_submit::builds::claimability::Claimability;
pub use crate::cli::release_submit::builds::worker::worker;
pub use crate::cli::release_submit::deliver::redelivery::entry::redeliver;
pub use crate::cli::release_submit::deliver::worker::delivery_worker;
pub use crate::cli::release_submit::run::finish::finish_ready_runs;
pub use crate::cli::release_submit::run::resume::resume;
pub use crate::cli::release_submit::run::submit::submit;

pub(crate) use crate::cli::release_submit::builds::jobs::platforms::{
    enqueue_platforms, refresh_build,
};
pub(crate) use crate::cli::release_submit::builds::jobs::terminal::terminal as terminal_job;
pub(crate) use crate::cli::release_submit::run::reports::{
    matching_runs, published_coordinates, recent_runs, RunFilter, VERSION_SCAN_WINDOW,
};
pub(crate) use crate::cli::release_submit::run::source::{
    build_identity, build_path, build_uri, committed_file, immutable, queue_immutable,
    resolve_commit, snapshot,
};
pub(crate) use crate::cli::release_submit::run::state::{
    load_build, persist_build_failure, save_build,
};

/// What a release is made from: a checkout to build first, or a build that
/// has already passed. Exactly one of the two; `--commit` and `--version`
/// belong to the checkout, so naming them beside `--build` is refused.
#[derive(Args)]
#[command(group = clap::ArgGroup::new("origin").required(true).args(["source", "build"]))]
pub struct ReleaseSubmitArgs {
    /// Build this checkout's committed tree first, then release that build.
    #[arg(long, requires = "version", conflicts_with = "build")]
    source: Option<PathBuf>,
    /// Read this full Git commit without changing or requiring a clean checkout.
    #[arg(long, requires = "source")]
    commit: Option<String>,
    /// The version the source declares; required with --source.
    #[arg(long, requires = "source")]
    version: Option<String>,
    /// Release a build that has already passed, by the id `stado build
    /// status` prints. A build that is still waiting or has failed is refused.
    #[arg(long)]
    build: Option<String>,
    #[arg(long, value_enum, default_value_t = SubmitChannel::Candidate)]
    channel: SubmitChannel,
    #[arg(long)]
    json: bool,
}

impl ReleaseSubmitArgs {
    /// The same submission `stado release submit` performs, for a checkout
    /// `release newest` has already read: it knows the commit and the version
    /// that commit declares, and `submit` reads both again and refuses if
    /// they disagree with what is passed here.
    pub(crate) fn for_checkout(
        source: &std::path::Path,
        commit: &str,
        version: &str,
        channel: SubmitChannel,
    ) -> Self {
        Self {
            source: Some(source.to_path_buf()),
            commit: Some(commit.to_string()),
            version: Some(version.to_string()),
            build: None,
            channel,
            json: false,
        }
    }
}

/// Resume recorded source and jobs without reading the current checkout.
#[derive(Args)]
pub struct ReleaseResumeArgs {
    /// Full run ID from release status --json.
    run_id: String,
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub struct ReleaseRedeliverArgs {
    product: String,
    run_id: String,
    delivery: String,
    /// Caller-retained idempotency token for this exact redelivery attempt.
    #[arg(long)]
    retry_token: String,
    #[arg(long)]
    json: bool,
}
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum SubmitChannel {
    Candidate,
    Stable,
}
impl From<SubmitChannel> for PipelineChannel {
    fn from(v: SubmitChannel) -> Self {
        match v {
            SubmitChannel::Candidate => Self::Candidate,
            SubmitChannel::Stable => Self::Stable,
        }
    }
}
#[derive(Args)]
pub struct ReleaseWorkerArgs {
    #[arg(long, default_value = "release-request.json")]
    request: PathBuf,
}
#[derive(Args)]
pub struct DeliveryWorkerArgs {
    #[arg(long, default_value = "delivery-request.json")]
    request: PathBuf,
}
