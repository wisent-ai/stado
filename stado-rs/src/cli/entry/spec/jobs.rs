//! The nested subcommand trees the queue verbs name: the stable machine
//! interface and the artifact registry. Recurring schedules are declared in
//! `schedules`.

use clap::Subcommand;

#[derive(Subcommand)]
pub(crate) enum MachineCommands {
    /// Submit one idempotent request from a JSON file.
    Submit {
        #[arg(long, required = true)]
        request_file: String,
    },
    /// Read one job directly by ID; `--until terminal` holds the read until
    /// the job ends.
    Status {
        job_id: String,
        #[arg(long, value_enum)]
        until: Option<StatusHold>,
    },
    /// Read the canonical command log from a byte cursor: to the end, or at
    /// most --limit bytes.
    Logs {
        job_id: String,
        #[arg(long, default_value_t = 0, allow_hyphen_values = true)]
        cursor: i64,
        #[arg(long, allow_hyphen_values = true)]
        limit: Option<i64>,
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

/// What `stado machine status --until` holds the read for.
#[derive(Clone, Copy, clap::ValueEnum)]
pub(crate) enum StatusHold {
    /// Answer once the job is completed, uploaded, failed or cancelled.
    Terminal,
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
    /// Delete an alias while it still targets --expected-target; the
    /// versions it pointed at stay. An absent alias is reported, not refused.
    Remove {
        /// The alias as a reference whose version part is the alias name.
        alias_ref: String,
        /// The version the alias must still target for the removal to happen.
        #[arg(long)]
        expected_target: String,
        #[arg(long)]
        json: bool,
    },
}
