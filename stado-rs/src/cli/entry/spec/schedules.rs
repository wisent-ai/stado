//! `stado schedule`: the recurring schedule verbs and the options of
//! `schedule create`.

use clap::Subcommand;

#[derive(Subcommand)]
pub(crate) enum ScheduleCommands {
    /// Create a recurring schedule that submits COMMAND on a cron schedule.
    Create(Box<ScheduleCreateArgs>),
    /// List all schedules.
    List {
        /// Emit the full persisted records as JSON, including an empty array.
        #[arg(long)]
        json: bool,
    },
    /// Print one schedule: its fields as text, or its full record with --json.
    Show {
        schedule_id: String,
        /// Print the full persisted record as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Change a schedule's command, cron expression or timezone; an enabled
    /// schedule's next run is recomputed from now.
    Edit {
        schedule_id: String,
        /// New command the schedule submits.
        #[arg(long)]
        command: Option<String>,
        /// New 5-field cron expression.
        #[arg(long)]
        cron: Option<String>,
        /// New IANA timezone the cron is interpreted in.
        #[arg(long)]
        tz: Option<String>,
        /// Emit the persisted schedule as JSON.
        #[arg(long)]
        json: bool,
    },
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
        /// Emit the submitted job identity as JSON.
        #[arg(long)]
        json: bool,
    },
}

/// `schedule create` options (boxed out of the enum to keep variant sizes
/// uniform — clippy::large_enum_variant).
#[derive(clap::Args)]
pub struct ScheduleCreateArgs {
    pub(crate) command: String,
    /// Caller-retained creation identity. Repeating the same declaration under
    /// it changes nothing and succeeds; a different declaration under it is
    /// refused naming what differs, never overwritten.
    #[arg(long)]
    pub(crate) id: Option<uuid::Uuid>,
    /// Emit the persisted schedule as JSON.
    #[arg(long)]
    pub(crate) json: bool,
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
    /// pip extras on the clone; empty (the default) skips install.
    #[arg(long, default_value = "")]
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
    /// Boot or container disk in GB for a machine rented to run each
    /// scheduled job. Without it no machine is rented; a registered host may
    /// still run the jobs.
    #[arg(long)]
    pub(crate) boot_disk_gb: Option<i64>,
    /// Seconds each scheduled job runs, as the submitter knows it; without it
    /// placement waits for a measured run of the same model on that hardware.
    #[arg(long)]
    pub(crate) runtime_seconds_estimate: Option<f64>,
    /// Scoped workload secret as ENV_NAME=SKARBIEC_ITEM#FIELD.
    #[arg(long = "secret-env")]
    pub(crate) secret_env: Vec<String>,
    /// skip (default): don't fire while the prior instance is
    /// still queued/running. allow: fire regardless.
    #[arg(long, value_enum, default_value_t = crate::schedules::OverlapPolicy::Skip)]
    pub(crate) overlap_policy: crate::schedules::OverlapPolicy,
    /// Create the schedule paused (enable later with `schedule resume`).
    #[arg(long)]
    pub(crate) disabled: bool,
}
