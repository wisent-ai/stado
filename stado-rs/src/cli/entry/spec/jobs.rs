//! The nested subcommand trees the queue verbs name: the stable machine
//! interface, the artifact registry, and recurring schedules.

use clap::Subcommand;

#[derive(Subcommand)]
pub(crate) enum MachineCommands {
    /// Submit one idempotent request from a JSON file.
    Submit {
        #[arg(long, required = true)]
        request_file: String,
    },
    /// Read one job directly by ID.
    Status { job_id: String },
    /// Read a byte-cursor page from the canonical command log.
    Logs {
        job_id: String,
        #[arg(long, default_value_t = 0, allow_hyphen_values = true)]
        cursor: i64,
        #[arg(long, default_value_t = 65536, allow_hyphen_values = true)]
        limit: i64,
    },
    /// Durably and idempotently cancel one job.
    Cancel { job_id: String },
    /// Download and verify canonical artifacts for a terminal job.
    Artifacts {
        job_id: String,
        #[arg(long, required = true)]
        output_dir: String,
    },
}

#[derive(Subcommand)]
pub(crate) enum ArtifactCommands {
    /// Build manifests from supported external artifact formats.
    #[command(subcommand)]
    Import(ArtifactImportCommands),
    /// List registered artifact versions.
    List {
        #[arg(long = "type", default_value = "")]
        type_name: String,
        #[arg(long, default_value = "")]
        namespace: String,
        #[arg(long, default_value = "")]
        name: String,
        /// Filter by KEY=VALUE.
        #[arg(long)]
        label: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// Show one version or resolve an alias.
    Show {
        r#ref: String,
        #[arg(long)]
        json: bool,
    },
    /// Resolve an alias to its immutable version.
    Resolve {
        r#ref: String,
        #[arg(long)]
        json: bool,
    },
    /// Validate and atomically publish a manifest JSON file.
    Publish {
        manifest_path: String,
        #[arg(long = "verify", overrides_with = "no_verify")]
        verify: bool,
        #[arg(long = "no-verify", overrides_with = "verify")]
        no_verify: bool,
        /// Run the adapter's exhaustive verification.
        #[arg(long)]
        full: bool,
        #[arg(long)]
        json: bool,
    },
    /// Manage mutable aliases that point at immutable versions.
    #[command(subcommand)]
    Alias(ArtifactAliasCommands),
    /// Re-run generic and type-specific verification.
    Verify {
        r#ref: String,
        #[arg(long)]
        full: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show producer and dependency provenance.
    Lineage {
        r#ref: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum ArtifactImportCommands {
    /// Verify and publish a pinned activation dataset revision.
    Activations {
        /// Hugging Face dataset, e.g. wisent-ai/activations.
        #[arg(long, required = true)]
        repo: String,
        /// Immutable Hugging Face commit SHA.
        #[arg(long, required = true)]
        revision: String,
        #[arg(long, required = true)]
        desired_state_dir: String,
        #[arg(long, default_value = "")]
        run_id: String,
        #[arg(long = "job-id")]
        job_ids: Vec<String>,
        #[arg(long, default_value = "")]
        version: String,
        #[arg(long)]
        alias: Vec<String>,
        #[arg(long)]
        full: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum ArtifactAliasCommands {
    /// Create an alias or update it with an optimistic precondition.
    Set {
        target_ref: String,
        alias: String,
        #[arg(long)]
        expected_previous: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum ScheduleCommands {
    /// Create a recurring schedule that submits COMMAND on a cron schedule.
    Create(Box<ScheduleCreateArgs>),
    /// List all schedules.
    List,
    /// Print a schedule's full JSON.
    Show { schedule_id: String },
    /// Delete a schedule (does not affect jobs it already submitted).
    Rm { schedule_id: String },
    /// Disable a schedule without deleting it.
    Pause { schedule_id: String },
    /// Re-enable a paused schedule (next run recomputed from now).
    Resume { schedule_id: String },
    /// Fire a schedule once with a caller-retained retry identity.
    Run {
        schedule_id: String,
        #[arg(long)]
        retry_token: String,
    },
}

/// `schedule create` options (boxed out of the enum to keep variant sizes
/// uniform — clippy::large_enum_variant).
#[derive(clap::Args)]
pub struct ScheduleCreateArgs {
    pub(crate) command: String,
    /// 5-field cron expression, e.g. "0 2 * * *" (daily 02:00).
    #[arg(long, required = true)]
    pub(crate) cron: String,
    /// IANA timezone the cron is interpreted in (default UTC).
    #[arg(long, default_value = "UTC")]
    pub(crate) tz: String,
    /// Optional provider constraint; Stado chooses when omitted.
    #[arg(long, default_value = "")]
    pub(crate) provider: String,
    /// Pin to --provider, or let any consumer claim (default).
    #[arg(long = "pin-provider", overrides_with = "any_provider")]
    pub(crate) pin_provider: bool,
    /// Let any consumer claim (default).
    #[arg(long = "any-provider", overrides_with = "pin_provider")]
    pub(crate) any_provider: bool,
    /// Dispatch on Spot/Preemptible GPUs.
    #[arg(long = "spot", overrides_with = "no_spot")]
    pub(crate) spot: bool,
    /// Do not dispatch on Spot/Preemptible GPUs (default).
    #[arg(long = "no-spot", overrides_with = "spot")]
    pub(crate) no_spot: bool,
    /// Hard $/hour cap (0 = none).
    #[arg(long, default_value_t = 0.0)]
    pub(crate) max_cost_per_hour: f64,
    /// Higher = scheduled first within FIFO bucket.
    #[arg(long, default_value_t = 0)]
    pub(crate) priority: i64,
    /// Pin accelerator label (e.g. 'nvidia-l4').
    #[arg(long, default_value = "")]
    pub(crate) gpu_type: String,
    /// Caller-declared VRAM (GB).
    #[arg(long, default_value_t = 0)]
    pub(crate) vram_gb: i64,
    /// Pin machine type verbatim.
    #[arg(long, default_value = "")]
    pub(crate) machine_type: String,
    /// Hard-pin every scheduled job to one registry target or consumer id.
    #[arg(long, default_value = "")]
    pub(crate) pinned_host: String,
    /// Git URL to clone before running.
    #[arg(long, default_value = "")]
    pub(crate) repo: String,
    /// Exact full lowercase commit to fetch; required with --repo.
    #[arg(long, default_value = "")]
    pub(crate) repo_ref: String,
    /// Override cloned-repo dir.
    #[arg(long, default_value = "")]
    pub(crate) repo_workdir: String,
    /// pip extras on the clone.
    #[arg(long, default_value = "train")]
    pub(crate) repo_extras: String,
    /// Shell snippet placed before the command in the same shell.
    #[arg(long, default_value = "")]
    pub(crate) pre_command: String,
    /// Comma-separated apt packages.
    #[arg(long, default_value = "")]
    pub(crate) apt: String,
    /// Additional provider-neutral `stado://` output destination.
    #[arg(long, default_value = "")]
    pub(crate) output_uri: String,
    /// Command that must exit 0 after success (reverses to FAILED otherwise).
    #[arg(long, default_value = "")]
    pub(crate) verify: String,
    /// Claim the whole GPU.
    #[arg(long)]
    pub(crate) exclusive: bool,
    /// Scoped workload secret as ENV_NAME=SKARBIEC_ITEM#FIELD.
    #[arg(long = "secret-env")]
    pub(crate) secret_env: Vec<String>,
    /// skip (default): don't fire while the prior instance is
    /// still queued/running. allow: fire regardless.
    #[arg(long, default_value = "skip", value_parser = ["skip", "allow"])]
    pub(crate) overlap_policy: String,
    /// Create the schedule paused (enable later with `schedule resume`).
    #[arg(long)]
    pub(crate) disabled: bool,
}
