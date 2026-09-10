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
mod deliver;
mod publish;
mod run;

pub use crate::cli::release_submit::builds::claimability::claimability;
pub use crate::cli::release_submit::builds::claimability::Claimability;
pub use crate::cli::release_submit::builds::worker::worker;
pub use crate::cli::release_submit::deliver::redelivery::entry::redeliver;
pub use crate::cli::release_submit::deliver::worker::delivery_worker;
pub use crate::cli::release_submit::run::resume::resume;
pub use crate::cli::release_submit::run::submit::submit;

pub(crate) use crate::cli::release_submit::run::reports::recent_runs;

#[derive(Args)]
pub struct ReleaseSubmitArgs {
    #[arg(long)]
    source: PathBuf,
    #[arg(long)]
    version: String,
    #[arg(long, value_enum, default_value_t = SubmitChannel::Candidate)]
    channel: SubmitChannel,
    #[arg(long)]
    json: bool,
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
