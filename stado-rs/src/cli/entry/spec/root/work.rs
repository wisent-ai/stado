//! Queue work and the worker that claims it: the second block of
//! `stado --help`.

use clap::Subcommand;

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

    /// Cancel a queued or running job.
    Cancel {
        job_id: String,
        /// Also delete the cloud instance the job is holding. Without it a
        /// cancelled job's VM keeps running, and billing.
        #[arg(long)]
        terminate: bool,
    },

    /// Rerun or watch one job.
    #[command(subcommand)]
    Job(job::JobCommands),

    /// Run local worker agent using live CPU, RAM, disk, and accelerator state.
    Agent {
        /// GPU type (auto-detected if --target/--auto absent)
        #[arg(long, default_value = "")]
        gpu_type: String,
        /// Pull the target's accelerator and policy from the registry by name.
        #[arg(long)]
        target: Option<String>,
        /// Look up self in registry by hostname; no manual config.
        #[arg(long)]
        auto: bool,
        /// Exit (and self-delete the GCE VM) when no jobs are active and no
        /// queued job is eligible. Use on ephemeral cloud VMs.
        #[arg(long)]
        idle_shutdown: bool,
        /// Consumer label in capacity broadcasts: "local" (physical box,
        /// default), "gcp" / "azure" / "aws" / "vast" (ephemeral cloud-agent VM).
        #[arg(long, default_value = "local")]
        kind: String,
        /// When the wisent-compute queue is empty, list this box on Vast.ai.
        /// Requires stado-vast/api_key in Skarbiec and WC_VAST_MACHINE_ID
        /// unless the machine can be discovered automatically.
        #[arg(long)]
        vast_auto_list: bool,
        /// Per-GPU-hour rental price USD when --vast-auto-list lists
        /// the box (default 0.50).
        #[arg(long, default_value_t = 0.50)]
        vast_price_gpu: f64,
        /// Cap the max rental length any Vast renter can buy from
        /// this offer (default 3600s = 1h). 0 to leave open-ended.
        #[arg(long, default_value_t = 3600)]
        vast_max_duration_s: i64,
    },
}
