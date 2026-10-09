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

    /// Serve Stado's read-only MCP tools to an agent: newline-delimited
    /// JSON-RPC on stdin, one answer per line on stdout. Every tool runs a
    /// read-only, non-spending `stado` subcommand of this same binary.
    Mcp,

    /// Submit a job (or batch) to the queue.
    Submit(Box<submit::SubmitArgs>),

    /// Show job status: the queue's jobs by state, or one job by its id.
    Status {
        /// A job id — whole (`job-` and its hex) or its first hex
        /// characters, with or without `job-` — read directly and, once its
        /// run was reaped, from the run's retained outcome; an id no job
        /// holds is refused by name. Any other text is a substring of a job
        /// id or batch id to filter the listing by.
        filter_id: Option<String>,
        /// Print the rows as JSON: each job's record with its lifecycle
        /// `state` and whether it was read from a reaped run.
        #[arg(long)]
        json: bool,
    },

    /// Download job results.
    Results { job_id: String, output_dir: String },

    /// Read or apply the formatting this product's own quality gate checks.
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
}

/// Worker options of `stado serve --worker`.
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
    /// Per-GPU-hour rental price in USD. Required when the Vast bridge runs;
    /// no price is built in.
    #[arg(long)]
    pub vast_price_gpu: Option<f64>,
    /// Maximum rental length in seconds; zero leaves it open-ended. Required
    /// when the Vast bridge runs.
    #[arg(long)]
    pub vast_max_duration_s: Option<i64>,
    /// Seconds Stado must be idle before the Vast bridge lists this host.
    /// Required when the bridge runs.
    #[arg(long)]
    pub vast_idle_window_s: Option<i64>,
    /// Seconds between queue polls when a poll started nothing. Required to
    /// run the worker; nothing in Stado chooses it.
    #[arg(long)]
    pub poll_seconds: Option<u64>,
}

/// The formatting gates a product declares, read or applied from a checkout.
#[derive(Subcommand)]
pub(crate) enum QualityCommands {
    /// Format this checkout the way its declared `fmt` gate reads it. A web
    /// product, whose gate is `stado web quality`, is refused: that gate runs
    /// the product's own scripts and names no formatter.
    Format {
        /// The checkout to format; the working directory by default.
        #[arg(long)]
        root: Option<String>,
    },
    /// Check that each committed Cargo.lock resolves its manifest (cargo
    /// metadata --locked, no compile), then run the declared `fmt` gate
    /// exactly as the release build runs it, writing nothing. On the web
    /// platform the gate is the declared `stado web quality`, run over the
    /// exported tree with the release worker's WISENT_* contract.
    Check {
        /// The checkout to check; the working directory by default.
        #[arg(long)]
        root: Option<String>,
    },
}
