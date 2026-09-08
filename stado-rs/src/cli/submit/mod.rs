//! `stado submit` — port of the `submit` command in `stado/cli.py`.
//!
//! One component per seam of the command: [`SubmitArgs`] below is the
//! command surface, `request` assembles the submission and dispatches the
//! batch, `identity` resolves the artifact refs, secret references and
//! pinned consumer a submission is keyed by, and `receipt` is the document
//! the command prints afterwards. Every name this module exposed before the
//! split is re-exported here, so `crate::cli::submit::NAME` still resolves.

use clap::Args;

mod identity;
mod receipt;
mod request;

pub use crate::cli::submit::request::dispatch::run;

pub(crate) use crate::cli::submit::identity::parse_secret_env;
pub(super) use crate::cli::submit::identity::resolve_pinned_host;

#[derive(Args, Debug)]
pub struct SubmitArgs {
    /// Shell command the job runs.
    command: String,

    /// Optional provider constraint (gcp/azure/aws/local); Stado chooses when omitted.
    #[arg(long, default_value = "")]
    provider: String,
    /// File with commands
    #[arg(long)]
    batch: Option<String>,
    /// Dispatch on Spot/Preemptible GPUs (cheaper, can be preempted).
    #[arg(long = "spot", overrides_with = "no_spot")]
    spot: bool,
    /// Do not dispatch on Spot/Preemptible GPUs (default).
    #[arg(long = "no-spot", overrides_with = "spot")]
    no_spot: bool,
    /// Hard cap on $/hour for the chosen accelerator. 0 = no cap.
    #[arg(long, default_value_t = 0.0)]
    max_cost_per_hour: f64,
    /// If true (default), any consumer with capacity can claim.
    #[arg(long = "any-provider", overrides_with = "pin_provider")]
    any_provider: bool,
    /// If --pin-provider, only the named --provider is allowed.
    #[arg(long = "pin-provider", overrides_with = "any_provider")]
    pin_provider: bool,
    /// Higher = scheduled first within FIFO bucket.
    #[arg(long, default_value_t = 0)]
    priority: i64,
    /// Hard RFC 3339 completion deadline used by autonomous placement.
    #[arg(long, default_value = "")]
    deadline_at: String,
    /// Optional git URL to clone before running command (no auth).
    #[arg(long, default_value = "")]
    repo: String,
    /// Exact full lowercase commit to fetch; required with --repo.
    #[arg(long, default_value = "")]
    repo_ref: String,
    /// Override cloned-repo dir; default = repo basename.
    #[arg(long, default_value = "")]
    repo_workdir: String,
    /// pip extras to install on the clone; empty skips install.
    #[arg(long, default_value = "train")]
    repo_extras: String,
    /// Pin the accelerator label (e.g. 'nvidia-l4', 'nvidia-a100-80gb').
    /// Skips the --model regex inference. Resolves machine_type from
    /// GPU_SIZING unless --machine-type is also passed.
    #[arg(long, default_value = "")]
    gpu_type: String,
    /// Caller-declared VRAM (GB). Picks the smallest SKU whose tier >= this value.
    /// Skips the --model regex inference.
    #[arg(long, default_value_t = 0)]
    vram_gb: i64,
    /// Pin the GCE/Azure machine type verbatim (e.g. 'g2-standard-8').
    /// Use for SKUs not in the wisent-compute catalog.
    #[arg(long, default_value = "")]
    machine_type: String,
    /// Shell snippet placed before the command in the SAME bash shell.
    /// Use to export env vars (LD_LIBRARY_PATH, CUDA_VISIBLE_DEVICES, etc.)
    /// that the command will see.
    #[arg(long, default_value = "")]
    pre_command: String,
    /// Comma-separated apt package list. Installed via sudo apt-get on
    /// cloud-kind agents only — local-kind agents refuse the job for safety.
    #[arg(long, default_value = "")]
    apt: String,
    /// Additional provider-neutral stado:// destination for job output.
    /// Additive — canonical status/<id>/output/ is always written too.
    #[arg(long, default_value = "")]
    output_uri: String,
    /// Shell command that must exit 0 after the job succeeds; non-zero
    /// reverses COMPLETED->FAILED. Catches silent-success failure modes.
    #[arg(long, default_value = "")]
    verify: String,
    /// Claim the whole GPU. The worker starts this job only while idle and
    /// admits no other job until it finishes. Use for diffusion training and
    /// full finetunes whose peak VRAM cannot be safely shared.
    #[arg(long)]
    exclusive: bool,
    /// Background job: the local worker may evict this job for a
    /// strictly-higher-priority queued job that does not otherwise fit.
    /// Requires --on-yield. The worker runs that hook (with WC_JOB_PID set),
    /// waits --yield-grace, then requeues the job.
    /// (resumes from wherever the hook saved state).
    #[arg(long)]
    yieldable: bool,
    /// Save-and-sync command run when the agent yields this job.
    /// Responsible for telling the job to stop, persisting state
    /// + artifacts (server/GCS/HF), and letting it exit. Required
    ///   with --yieldable.
    #[arg(long, default_value = "")]
    on_yield: String,
    /// Seconds the --on-yield hook + clean exit get before the
    /// agent SIGKILLs the process group (default 120).
    #[arg(long, default_value_t = 120)]
    yield_grace: i64,
    /// Pinned artifact input as NAME=TYPE/NAMESPACE/NAME@VERSION_OR_ALIAS.
    #[arg(long = "input-artifact")]
    input_artifacts: Vec<String>,
    /// Scoped workload secret as ENV_NAME=SKARBIEC_ITEM#FIELD.
    #[arg(long = "secret-env")]
    secret_env: Vec<String>,
    /// Apply a named profile from the bundled profiles dir (or
    /// $WC_PROFILES_DIR). CLI flags override profile fields.
    /// Run `stado profiles` to list available profiles.
    #[arg(long, default_value = "")]
    profile: String,
    /// Hard-pin this job to one consumer: a registry target
    /// name (resolved to kind-hostname) or a raw consumer_id.
    /// Only that consumer may claim the job; the makespan
    /// matcher never reassigns it.
    #[arg(long, default_value = "")]
    pinned_host: String,
    /// Stable caller-retained identity for exactly-once submission. Required:
    /// repeating the same run id and request returns the original jobs.
    #[arg(long)]
    run_id: String,
}
