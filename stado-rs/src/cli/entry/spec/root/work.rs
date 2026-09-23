//! Queue work and the worker that claims it: the second block of
//! `stado --help`.

use clap::{Args, Subcommand};

use crate::cli::*;

/// The second block of `stado` verbs. Flattened into
/// `super::super::Commands`, so splitting the declaration across files
/// changes no command line.
#[derive(Subcommand)]
pub(crate) enum WorkCommands {
    /// Stable JSON machine interface.
    #[command(subcommand)]
    Machine(MachineCommands),

    /// Submit a job (or batch) to the queue.
    Submit(Box<submit::SubmitArgs>),

    /// Show job status.
    Status {
        /// Job id (8 hex chars) or batch id substring to filter by.
        filter_id: Option<String>,
    },

    /// Download job results.
    Results { job_id: String, output_dir: String },

    /// Apply the formatting this product's own quality gate checks.
    #[command(subcommand)]
    Quality(QualityCommands),

    /// Cancel a queued or running job, or every job still waiting in the
    /// queue.
    Cancel {
        /// The job to cancel. Omitted with `--queued`, which selects them all.
        job_id: Option<String>,
        /// Cancel every job still in the queue, claimed by nobody. A fleet
        /// that has queued work it no longer wants had to be emptied one id
        /// at a time, which is how a queue stays full.
        #[arg(long)]
        queued: bool,
        /// Also delete the cloud instance the job is holding. Without it a
        /// cancelled job's VM keeps running, and billing.
        #[arg(long)]
        terminate: bool,
    },

    /// Rerun or watch one job.
    #[command(subcommand)]
    Job(job::JobCommands),

    /// Run local worker agent using live CPU, RAM, disk, and accelerator state.
    Agent(AgentOptions),
}

/// Worker options shared by the standalone worker and the host service.
#[derive(Args, PartialEq)]
pub(crate) struct AgentOptions {
    /// GPU type (auto-detected if --target/--auto absent).
    #[arg(long, default_value = "")]
    pub gpu_type: String,
    /// Pull the target's accelerator and policy from the registry by name.
    #[arg(long)]
    pub target: Option<String>,
    /// Look up self in registry by hostname; no manual config.
    #[arg(long)]
    pub auto: bool,
    /// Exit and retire an ephemeral cloud VM when no eligible work remains.
    #[arg(long)]
    pub idle_shutdown: bool,
    /// Consumer label: local, gcp, azure, aws, or vast.
    #[arg(long, default_value = "local")]
    pub kind: String,
    /// List idle capacity on Vast.ai using its existing Skarbiec grant.
    #[arg(long)]
    pub vast_auto_list: bool,
    /// Per-GPU-hour rental price in USD.
    #[arg(long, default_value_t = 0.50)]
    pub vast_price_gpu: f64,
    /// Maximum rental length in seconds; zero leaves it open-ended.
    #[arg(long, default_value_t = 3600)]
    pub vast_max_duration_s: i64,
}

/// The gates a product declares, applied rather than only read.
#[derive(Subcommand)]
pub(crate) enum QualityCommands {
    /// Format this checkout the way its declared `fmt` gate reads it.
    Format {
        /// The checkout to format; the working directory by default.
        #[arg(long)]
        root: Option<String>,
    },
}
