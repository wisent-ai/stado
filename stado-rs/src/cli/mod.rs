//! CLI entry point: submit, status, results, cancel, profiles, config.
//!
//! Port of `stado/cli.py` (click) to clap derive. The full command tree is
//! declared and every branch dispatches to its Rust implementation.
//!
//! Implemented and wired to the library: `package-root`, `capabilities`,
//! `submit`, `status`, `cancel`, `results`, `profiles`, `config`, `schedule`,
//! `artifact`, `cost`, `vast`, `agent`, `disk-cleanup`, `resources`,
//! `install-disk-cleanup`, `bootstrap`, `recovery`, the complete `host`,
//! `registry`, and `quota` groups, plus coordinator and dashboard control planes.

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

pub mod agent;
pub mod alerts;
pub mod artifact;
pub mod autonomy_cmd;
pub mod azure;
pub mod billing;
pub mod blast_radius;
pub mod bootstrap;
pub mod builds;
pub mod cancel;
pub mod capabilities;
pub mod cloudflare;
pub mod coding;
pub mod config_cmd;
pub mod control_plane;
pub mod coordinator;
pub mod cost;
pub mod dashboard;
pub mod database;
pub mod directory;
pub mod disk_cleanup;
pub mod doctor;
pub mod egress;
pub mod fleet;
pub mod host;
pub mod identity;
pub mod inference;
pub mod instances;
pub mod job;
pub mod machine;
pub mod mail;
pub mod overview;
pub mod placement;
pub mod precheck_runner;
pub mod product;
pub mod profiles_cmd;
pub mod queue;
pub mod quota;
pub mod recovery;
pub mod registry;
pub mod release_catalog;
pub mod release_cmd;
pub mod release_evidence;
pub mod release_quarantine;
pub mod release_submit;
pub mod resolver;
pub mod resources;
pub mod results;
pub mod schedule;
pub mod secrets;
pub mod service;
pub mod service_converge;
pub mod service_verify;
pub mod status;
pub mod storage;
pub mod stream;
pub mod submit;
pub mod table;
pub mod vast;

/// Command failure with a click-matching exit code. A `Some` message is
/// printed as `Error: {msg}` on stderr (click `ClickException`, code 1)
/// followed by the classified operator line, and the process exits with
/// [`crate::failure::FailureCode::exit_code`] applied to `code`; a `None`
/// message exits silently (click `SystemExit`, e.g. config validation
/// failure after the ERROR lines were already printed).
#[derive(Debug)]
pub struct CmdError {
    pub message: Option<String>,
    pub code: i32,
}

/// click `ClickException`'s exit code: "it ran and failed". Every runtime
/// failure has used it since the Python original, and it stays the default —
/// only a retryable failure is remapped, in [`main_entry`].
pub const CLICK_ERROR_CODE: i32 = true as i32;

impl CmdError {
    /// click `ClickException`: "Error: {msg}" on stderr, exit 1.
    pub fn click(msg: impl Into<String>) -> Self {
        Self {
            message: Some(msg.into()),
            code: CLICK_ERROR_CODE,
        }
    }

    /// click `UsageError`: "Error: {msg}" on stderr, exit 2 — the code
    /// click reserves for "you invoked this wrongly", as distinct from
    /// [`Self::click`]'s "it ran and failed".
    pub fn usage(msg: impl Into<String>) -> Self {
        Self {
            message: Some(msg.into()),
            // click's UsageError.exit_code, as a ratio of two width
            // constants rather than a bare literal.
            code: (u16::BITS / u8::BITS) as i32,
        }
    }

    /// click `SystemExit(code)`: nothing more to print.
    pub fn silent(code: i32) -> Self {
        Self {
            message: None,
            code,
        }
    }
}

impl std::fmt::Display for CmdError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.message.as_deref() {
            Some(message) => formatter.write_str(message),
            None => write!(formatter, "command failed with exit code {}", self.code),
        }
    }
}

impl std::error::Error for CmdError {}

impl From<String> for CmdError {
    fn from(msg: String) -> Self {
        Self::click(msg)
    }
}

impl From<&str> for CmdError {
    fn from(msg: &str) -> Self {
        Self::click(msg)
    }
}

impl From<crate::queue::submit::SubmitError> for CmdError {
    fn from(exc: crate::queue::submit::SubmitError) -> Self {
        Self::click(exc.to_string())
    }
}

impl From<crate::queue::StorageError> for CmdError {
    fn from(exc: crate::queue::StorageError) -> Self {
        Self::click(exc.to_string())
    }
}

impl From<crate::profiles::ProfileError> for CmdError {
    fn from(exc: crate::profiles::ProfileError) -> Self {
        Self::click(exc.to_string())
    }
}

impl From<crate::config_file::ConfigError> for CmdError {
    fn from(exc: crate::config_file::ConfigError) -> Self {
        Self::click(exc.to_string())
    }
}

impl From<serde_json::Error> for CmdError {
    fn from(exc: serde_json::Error) -> Self {
        Self::click(exc.to_string())
    }
}

impl From<std::io::Error> for CmdError {
    fn from(exc: std::io::Error) -> Self {
        Self::click(exc.to_string())
    }
}

impl From<reqwest::Error> for CmdError {
    fn from(exc: reqwest::Error) -> Self {
        Self::click(exc.to_string())
    }
}

impl From<crate::providers::ProviderError> for CmdError {
    fn from(exc: crate::providers::ProviderError) -> Self {
        Self::click(exc.to_string())
    }
}

const ONBOARDING: &str = "\
Stado — one queue for every machine.

Stado needs three things:
- state storage for the queue and results,
- at least one compute provider,
- a running worker that can claim jobs.

Fastest path: local mode. `stado config init` creates:
- provider: local
- queue storage: ~/.stado/local-storage
- backup storage: ~/.stado/local-backup

No cloud account or credentials are required for local mode.
The worker host must already have the shell, runtime, and GPU driver required by the workload.

1. Create the local configuration:
   stado config init

2. Check the installation:
   stado config validate
   stado doctor --fix-hints

3. Start the local control plane:
   stado local-control-plane

Submit your first job:
   stado submit \"printf 'hello from Stado\\n'\"

Already configured? Run:
   stado overview

More commands:
   stado --help
";

fn print_onboarding() {
    print!("{ONBOARDING}");
}

#[derive(Parser)]
#[command(
    version,
    about = "Stado — policy-controlled queue and compute control plane."
)]
pub struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Print the installed crate data root for desktop provisioning.
    #[command(name = "package-root", hide = true)]
    PackageRoot,

    /// List Stado capability families, variants, providers and active selections.
    Capabilities {
        /// Restrict output to one capability family.
        capability: Option<String>,
        /// Emit the versioned machine-readable catalog.
        #[arg(long)]
        json: bool,
    },

    /// One operator snapshot: jobs, active workers, quota, budgets, burn and credits.
    Overview {
        /// Emit the complete machine-readable snapshot.
        #[arg(long)]
        json: bool,
    },

    /// Inventory a dependency's live resources, auth, consumers, storage, and DR coverage.
    #[command(name = "blast-radius")]
    BlastRadius(blast_radius::BlastRadiusArgs),

    /// Inventory, plan, execute, verify, and restore resource operations.
    #[command(subcommand)]
    Resources(resources::ResourcesCommands),
    /// Inspect and control autonomous placement and resource reconciliation.
    #[command(subcommand)]
    Optimize(autonomy_cmd::OptimizeCommands),

    /// Inspect or refresh cross-cloud costs, grants, burn, and credit balances.
    #[command(subcommand)]
    Billing(BillingCommands),

    /// Authenticate an Azure operator and repair the Stado RBAC contract.
    #[command(subcommand)]
    Azure(azure::AzureCommands),

    /// Configure Cloudflare Tunnel ingress and DNS through Stado-held credentials.
    #[command(subcommand)]
    Cloudflare(cloudflare::CloudflareCommands),

    /// Search and deterministically analyze Gmail messages without modifying them.
    #[command(subcommand)]
    Mail(MailCommands),

    /// Run registry-authorized cleanup for this local target.
    #[command(name = "disk-cleanup")]
    DiskCleanup {
        /// Run one interval-gated cleanup check (default).
        #[arg(long)]
        once: bool,
        /// Continuously check at the canonical policy interval.
        #[arg(long)]
        watch: bool,
        /// Plan a pass and delete nothing: same policy, same scan, an
        /// `enforce` policy pinned to the janitor's own report mode.
        #[arg(long)]
        dry_run: bool,
    },

    /// Install the registry-controlled cleanup watch on this Mac.
    #[command(name = "install-disk-cleanup")]
    InstallDiskCleanup,

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

    /// Run local GPU agent. Polls queue, respects Vast.ai renters.
    Agent {
        /// GPU type (auto-detected if --target/--auto absent)
        #[arg(long, default_value = "")]
        gpu_type: String,
        /// Pull gpu_type/slots from registry by name.
        #[arg(long)]
        target: Option<String>,
        /// Look up self in registry by hostname; no manual config.
        #[arg(long)]
        auto: bool,
        /// Exit (and self-delete the GCE VM) when no slots active and no
        /// queued job is eligible. Use on ephemeral cloud-VM agents.
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

    /// Run the provider-neutral scheduling tick locally.
    ///
    /// Reads cadence and identity from the named coordinator entry. Queue,
    /// registry, capacity, and schedule state use the configured Stado
    /// storage backend.
    Coordinator {
        /// Coordinator name or host heuristic (default: active=true entry).
        #[arg(long)]
        target: Option<String>,
        /// Run a single scheduling tick and exit (cron-friendly).
        #[arg(long)]
        once: bool,
    },

    /// Run the Stado API listener for the wisent-compute queue.
    ///
    /// Serves the authenticated object, release, machine, service and
    /// host-health routes plus the three enrollment routes over loopback
    /// HTTP. It serves no HTML page; the operator workspace is Stado
    /// Desktop.
    ///
    /// With --enrollment-only the listener serves nothing but the three
    /// enrollment routes, which is the only shape safe to publish.
    Dashboard {
        /// Bind address. Default WC_DASHBOARD_BIND or 127.0.0.1.
        #[arg(long)]
        bind: Option<String>,
        /// Port. Default WC_DASHBOARD_PORT or 8765.
        #[arg(long)]
        port: Option<i64>,
        /// Serve ONLY GET /join.sh, GET /api/fleet/invite/key and
        /// POST /api/fleet/join; answer 404 to every other path and method.
        /// Publish this listener through a tunnel, never the full dashboard.
        #[arg(long)]
        enrollment_only: bool,
    },

    /// Run a device-local API listener, scheduler, and worker.
    #[command(name = "local-control-plane", hide = true)]
    LocalControlPlane {
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
        #[arg(long, default_value_t = 8765)]
        port: i64,
        #[arg(long, default_value_t = 15)]
        interval: i64,
    },

    /// Run a cloud-hosted coordinator and API listener.
    #[command(name = "cloud-control-plane", hide = true)]
    CloudControlPlane {
        #[arg(long, default_value = "localhost")]
        bind: String,
        #[arg(long, default_value_t = 8080)]
        port: i64,
        #[arg(long, default_value_t = 30)]
        interval: i64,
    },

    /// GPU quota inspection and increase requests across WC_PROVIDERS.
    ///
    /// Default (no subcommand) is equivalent to `quota show` — prints
    /// live cloud quota minus reservation minus running per provider.
    Quota {
        /// Emit machine-readable JSON instead of the table (show subcommand).
        #[arg(long)]
        json: bool,
        #[command(subcommand)]
        sub: Option<QuotaCommands>,
    },

    /// List available submit profiles, or show one profile's JSON.
    Profiles { name: Option<String> },

    /// Inspect or change stado configuration: show | validate | init | set.
    Config {
        #[arg(default_value = "show")]
        sub: String,
        /// `set`: dotted key, e.g. `alerts.channels`.
        key: Option<String>,
        /// `set`: JSON value; a bare word is stored as a string.
        value: Option<String>,
    },

    /// Publish and consume immutable, versioned artifacts.
    #[command(subcommand)]
    Artifact(ArtifactCommands),

    /// Build once, sign, promote, roll out, and roll back product releases.
    #[command(subcommand)]
    Release(release_cmd::ReleaseCommands),

    /// Manage recurring (cron) jobs — submit a command on a cron schedule.
    ///
    /// A schedule is evaluated every coordinator tick; when due, the
    /// coordinator submits a fresh job with the same routing/sizing and
    /// secret-reference contract. Schedules live in configured Stado storage.
    #[command(subcommand)]
    Schedule(ScheduleCommands),

    /// Per-job and per-batch cost reporting from observed wall-times.
    #[command(subcommand)]
    Cost(CostCommands),

    /// Manage the canonical compute-target registry in configured Stado storage.
    #[command(subcommand)]
    Registry(RegistryCommands),

    /// Manage native build recipes: poll a repo, build on new commits.
    #[command(subcommand)]
    Builds(builds::BuildsCommands),

    /// Add machines to the fleet, group them, hold their SSH keys, and
    /// diagnose the workers: enroll, join/approve, key, doctor.
    #[command(subcommand)]
    Fleet(fleet::FleetCommands),

    /// Which host holds which identity, and whether that is still true.
    #[command(subcommand)]
    Identity(IdentityCommands),

    /// Manage operating-system resources on registry hosts.
    #[command(subcommand)]
    Host(HostCommands),

    /// Provision wisent-compute services persistently across reboots.
    Bootstrap {
        /// Specific entry name (target or coordinator).
        #[arg(long)]
        target: Option<String>,
        /// Print unit/plist; do not enable.
        #[arg(long)]
        dry_run: bool,
        /// Install on THIS machine (launchd/systemd --user) instead of via SSH.
        #[arg(long)]
        local: bool,
    },

    /// Vast.ai marketplace host-listing (rent our idle GPU).
    #[command(subcommand)]
    Vast(VastCommands),

    /// Inspect and reap live agent VMs across the configured cloud providers.
    #[command(subcommand)]
    Instances(instances::InstancesCommands),

    /// Transactional outage recovery: fence, migrate, verify, and cut over.
    #[command(subcommand)]
    Recovery(recovery::RecoveryCommands),

    /// Move queue state between storage backends (billing-outage migration).
    #[command(subcommand)]
    Storage(storage::StorageCommands),
    /// Read, migrate, and manage application credentials in the selected store.
    #[command(name = "credentials", visible_alias = "secrets", subcommand)]
    Secrets(secrets::SecretsCommands),
    /// Maintenance mode: pause/resume dispatching, and drain the fleet.
    #[command(subcommand)]
    Queue(queue::QueueCommands),
    /// Show which alert channels resolve, and page them on purpose.
    #[command(subcommand)]
    Alerts(alerts::AlertsCommands),
    /// Manage the services registry hosts run: list, status, restart,
    /// adopt, retire, deploy, logs, env.
    #[command(subcommand)]
    Service(service::ServiceCommands),
    /// Run host-local network egress processes under Stado service management.
    #[command(subcommand)]
    Egress(egress::EgressCommands),
    /// Install, inspect, update, roll back and remove canonical Wisent products.
    #[command(subcommand)]
    Product(product::ProductCommands),
    /// Atomically relocate a declared service group between registered hosts.
    #[command(subcommand)]
    Placement(placement::PlacementCommands),
    /// Resolve logical services and run the local Stado data plane.
    #[command(subcommand)]
    Resolver(resolver::ResolverCommands),
    /// Resolve fleet databases: placement endpoint and credential coordinate.
    #[command(subcommand)]
    Database(database::DatabaseCommands),
    /// Plan, deploy, route and operate local OpenAI-compatible inference.
    ///
    /// Being replaced by the service declaration contract: a model server is
    /// a service like any other, declared once with `stado service declare`
    /// and deployed with `stado service deploy`. This plane keeps working
    /// while its declarations migrate; add nothing new to it.
    #[command(subcommand)]
    Inference(inference::InferenceCommands),
    /// Provision and operate an interactive display session on a host, and
    /// stream it to a client (Moonlight): the way to use a fleet GPU
    /// interactively, since a board cannot be borrowed over a network.
    #[command(subcommand)]
    Stream(stream::StreamCommands),
    /// Ordered deployment preflight: config, storage, provider auth, quota,
    /// release channel, agent template, VM identity, registry, queue pause
    /// state and alert channels. Exits non-zero if any check FAILs.
    Doctor(doctor::DoctorArgs),
}

#[derive(Subcommand)]
pub(crate) enum BillingCommands {
    /// Read the last billing snapshot published by the coordinator.
    Show {
        #[arg(long)]
        json: bool,
    },
    /// Query billing providers now and publish a fresh snapshot.
    Refresh {
        #[arg(long)]
        json: bool,
    },
    /// Foreground billing watchdog: poll, evaluate credit balance AND
    /// account health, and alert on transitions. Deliberately runnable
    /// outside the cloud it monitors (see `cli/billing.rs` module docs).
    Watch {
        /// Poll interval as a duration string: 45s, 5m, 2h, 1d.
        #[arg(long, default_value = "5m", value_parser = billing::parse_interval)]
        interval: std::time::Duration,
        /// Evaluate once and exit instead of looping.
        #[arg(long)]
        once: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum MailCommands {
    /// Search Gmail and list categorized message metadata.
    Search {
        /// Gmail search expression, for example: from:microsoft.com azure.
        #[arg(long, default_value = "")]
        query: String,
        /// Maximum messages to read.
        #[arg(long, default_value_t = default_mail_results())]
        max_results: usize,
        #[arg(long)]
        json: bool,
    },
    /// Aggregate categories, financial amounts, dates, links, and required actions.
    Analyze {
        /// Gmail search expression.
        #[arg(long, default_value = "")]
        query: String,
        /// Maximum messages to read.
        #[arg(long, default_value_t = default_mail_results())]
        max_results: usize,
        #[arg(long)]
        json: bool,
    },
}

fn default_mail_results() -> usize {
    usize::try_from(u8::BITS).expect("u8 bit width fits usize")
}

#[derive(Subcommand)]
enum MachineCommands {
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
enum QuotaCommands {
    /// Show GPU quota totals across all providers in WC_PROVIDERS.
    Show {
        /// Emit machine-readable JSON instead of the table.
        #[arg(long)]
        json: bool,
    },
    /// Submit GPU quota-increase request(s) for ACCEL via the provider Quotas API.
    Request {
        accel: String,
        /// New per-region quota limit to request (e.g. 16).
        #[arg(long = "to", required = true)]
        new_limit: i64,
        /// Comma-separated regions/locations; default = every region
        /// the provider dispatches into (REGIONS / AZURE_LOCATIONS).
        #[arg(long, default_value = "")]
        region: String,
        /// Comma-separated provider list (gcp,azure); default = WC_PROVIDERS.
        #[arg(long, default_value = "")]
        provider: String,
        /// Reviewer-visible justification text.
        #[arg(
            long,
            default_value = "wisent-compute autoscaler queue depth requires more parallel GPU capacity"
        )]
        justification: String,
        /// Contact email for the Cloud Quotas reviewer (required for GCP).
        /// Default: $WC_QUOTA_CONTACT_EMAIL.
        #[arg(long, default_value = "")]
        email: String,
        /// Emit machine-readable JSON result list.
        #[arg(long)]
        json: bool,
    },
    /// Respond to Open Azure quota support tickets awaiting customer info.
    #[command(name = "azure-replies")]
    AzureReplies {
        /// Print what would be sent without invoking az
        /// support communication create.
        #[arg(long)]
        dry_run: bool,
        /// Contact email shown in the response signature.
        /// Default: $WC_QUOTA_CONTACT_EMAIL.
        #[arg(long, default_value = "")]
        email: String,
    },
    /// Post a credit-funded-subscription escalation reply on every
    /// Open Azure quota ticket whose latest Microsoft message was a
    /// billing-side denial.
    #[command(name = "azure-escalate")]
    AzureEscalate {
        /// Print what would be sent without invoking az
        /// support communication create.
        #[arg(long)]
        dry_run: bool,
        /// Contact email shown in the response signature.
        /// Default: $WC_QUOTA_CONTACT_EMAIL.
        #[arg(long, default_value = "")]
        email: String,
    },
    /// List the full GPU catalog for each provider in WC_PROVIDERS.
    Catalog {
        /// Comma-separated provider list (gcp,azure); default = WC_PROVIDERS.
        #[arg(long, default_value = "")]
        provider: String,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Submit quota-increase requests for EVERY known GPU family on each provider.
    #[command(name = "request-all")]
    RequestAll {
        /// New per-region quota limit to request for every GPU family.
        #[arg(long = "to", required = true)]
        new_limit: i64,
        /// Comma-separated provider list (gcp,azure); default = WC_PROVIDERS.
        #[arg(long, default_value = "")]
        provider: String,
        /// Comma-separated regions/locations; default = the provider's
        /// configured REGIONS / AZURE_LOCATIONS.
        #[arg(long, default_value = "")]
        region: String,
        /// Reviewer-visible justification text.
        #[arg(
            long,
            default_value = "wisent-compute autoscaler bulk capacity request: provision GPU headroom across every supported family in the dispatch regions so the scheduler can fall through to whichever family Google/Azure can serve."
        )]
        justification: String,
        /// Contact email for the GCP Cloud Quotas reviewer.
        /// Default: $WC_QUOTA_CONTACT_EMAIL.
        #[arg(long, default_value = "")]
        email: String,
        /// Emit machine-readable JSON result list.
        #[arg(long)]
        json: bool,
    },
    /// Cross-provider in-flight quota requests + support communications.
    Requests {
        /// Comma-separated provider list (gcp,azure); default = WC_PROVIDERS.
        #[arg(long, default_value = "")]
        provider: String,
        /// Filter GCP rows by state (reconciling, approved, denied,
        /// partially_approved, unknown); empty = all.
        #[arg(long, default_value = "")]
        state: String,
        /// For Azure, only show tickets where Microsoft has the
        /// latest message and is awaiting a customer reply.
        #[arg(long)]
        awaiting_customer: bool,
        /// Emit machine-readable JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ArtifactCommands {
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
enum ArtifactImportCommands {
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
enum ArtifactAliasCommands {
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
enum ScheduleCommands {
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
    /// Fire a schedule once right now, regardless of its next run time.
    Run { schedule_id: String },
}

/// `schedule create` options (boxed out of the enum to keep variant sizes
/// uniform — clippy::large_enum_variant).
#[derive(clap::Args)]
pub struct ScheduleCreateArgs {
    command: String,
    /// 5-field cron expression, e.g. "0 2 * * *" (daily 02:00).
    #[arg(long, required = true)]
    cron: String,
    /// IANA timezone the cron is interpreted in (default UTC).
    #[arg(long, default_value = "UTC")]
    tz: String,
    /// Optional provider constraint; Stado chooses when omitted.
    #[arg(long, default_value = "")]
    provider: String,
    /// Pin to --provider, or let any consumer claim (default).
    #[arg(long = "pin-provider", overrides_with = "any_provider")]
    pin_provider: bool,
    /// Let any consumer claim (default).
    #[arg(long = "any-provider", overrides_with = "pin_provider")]
    any_provider: bool,
    /// Dispatch on Spot/Preemptible GPUs.
    #[arg(long = "spot", overrides_with = "no_spot")]
    spot: bool,
    /// Do not dispatch on Spot/Preemptible GPUs (default).
    #[arg(long = "no-spot", overrides_with = "spot")]
    no_spot: bool,
    /// Hard $/hour cap (0 = none).
    #[arg(long, default_value_t = 0.0)]
    max_cost_per_hour: f64,
    /// Higher = scheduled first within FIFO bucket.
    #[arg(long, default_value_t = 0)]
    priority: i64,
    /// Pin accelerator label (e.g. 'nvidia-l4').
    #[arg(long, default_value = "")]
    gpu_type: String,
    /// Caller-declared VRAM (GB).
    #[arg(long, default_value_t = 0)]
    vram_gb: i64,
    /// Pin machine type verbatim.
    #[arg(long, default_value = "")]
    machine_type: String,
    /// Hard-pin every scheduled job to one registry target or consumer id.
    #[arg(long, default_value = "")]
    pinned_host: String,
    /// Git URL to clone before running.
    #[arg(long, default_value = "")]
    repo: String,
    /// Exact full lowercase commit to fetch; required with --repo.
    #[arg(long, default_value = "")]
    repo_ref: String,
    /// Override cloned-repo dir.
    #[arg(long, default_value = "")]
    repo_workdir: String,
    /// pip extras on the clone.
    #[arg(long, default_value = "train")]
    repo_extras: String,
    /// Shell snippet placed before the command in the same shell.
    #[arg(long, default_value = "")]
    pre_command: String,
    /// Comma-separated apt packages.
    #[arg(long, default_value = "")]
    apt: String,
    /// Additional provider-neutral `stado://` output destination.
    #[arg(long, default_value = "")]
    output_uri: String,
    /// Command that must exit 0 after success (reverses to FAILED otherwise).
    #[arg(long, default_value = "")]
    verify: String,
    /// Claim the whole GPU.
    #[arg(long)]
    exclusive: bool,
    /// Scoped workload secret as ENV_NAME=SKARBIEC_ITEM#FIELD.
    #[arg(long = "secret-env")]
    secret_env: Vec<String>,
    /// skip (default): don't fire while the prior instance is
    /// still queued/running. allow: fire regardless.
    #[arg(long, default_value = "skip", value_parser = ["skip", "allow"])]
    overlap_policy: String,
    /// Create the schedule paused (enable later with `schedule resume`).
    #[arg(long)]
    disabled: bool,
}

#[derive(Subcommand)]
pub(crate) enum CostCommands {
    /// Summarize $ spent per target_kind and per model from completed jobs.
    Report,
    /// Project total $ for a batch file using observed per-job cost.
    Estimate { batch_file: String },
    /// Show the attributed provider/owner/workload cost ledger.
    Allocation {
        #[arg(long)]
        json: bool,
    },
    /// Show current burn, month-end projection, budget, and credit runway.
    Forecast {
        #[arg(long)]
        json: bool,
    },
    /// Show active cost and resource anomalies.
    Anomalies {
        #[arg(long)]
        json: bool,
    },
    /// Show predicted versus realized savings.
    Savings {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum RegistryCommands {
    /// Validate a local registry-v2 JSON document.
    Validate { path: Option<String> },
    /// Upload local registry.json to the canonical registry object.
    Push {
        /// The document to upload, or `-` to read it from stdin. With neither,
        /// the repository's bundled registry is uploaded - which is what
        /// erased the canonical document on 2026-09-01 when a caller piped a
        /// body this command never reads.
        path: Option<String>,
        /// Allow a write that deletes a top-level key the canonical document
        /// still carries. Without this the upload is refused and names them.
        /// It does NOT allow a write that erases every target.
        #[arg(long)]
        force: bool,
        /// Allow a write that leaves the registry with no targets at all.
        /// Separate from --force on purpose: every other guard asks whether a
        /// deletion was meant, and this one asks whether the document is a
        /// fleet at all.
        #[arg(long)]
        allow_empty_fleet: bool,
    },
    /// Print the canonical registry to stdout.
    Pull,
    /// Print which registry target is this machine.
    #[command(name = "self")]
    SelfTarget {
        /// Print only the target name, for scripts.
        #[arg(long)]
        name_only: bool,
    },
    /// Diff registry declarations against live host state.
    Doctor {
        /// Emit the findings as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Manage hosts in the canonical registry.
    #[command(subcommand)]
    Host(RegistryHostCommands),
    /// Table of every registry host and its last beacon, worst first.
    #[command(name = "beacon-age")]
    BeaconAge {
        /// Emit the table as JSON.
        #[arg(long)]
        json: bool,
    },
}

fn parse_target_kind(raw: &str) -> Result<String, String> {
    crate::capabilities::configurable_variant(crate::capabilities::RuntimeFacet::HostTarget, raw)
        .map(|variant| variant.id.to_string())
        .ok_or_else(|| {
            let choices = crate::capabilities::configurable_ids(
                crate::capabilities::RuntimeFacet::HostTarget,
            )
            .collect::<Vec<_>>()
            .join(", ");
            format!("unknown target kind {raw:?}; use one of: {choices}")
        })
}

fn parse_release_platform(raw: &str) -> Result<String, String> {
    crate::deploy::products::managed_platform(raw)
        .map(str::to_string)
        .map_err(|error| error.to_string())
}

#[derive(Subcommand)]
enum RegistryHostCommands {
    /// Onboard HOST into the canonical registry, validated.
    Add {
        host: String,
        /// SSH destination ([user@]host[:port]) the fleet reaches HOST at.
        #[arg(long)]
        ssh: String,
        /// Registry target kind.
        #[arg(long, default_value = "local", value_parser = parse_target_kind)]
        kind: String,
        /// Release platform confirmed during enrollment.
        #[arg(long, value_parser = parse_release_platform)]
        release_platform: String,
    },
    /// Manage ordered SSH connection paths for an existing host.
    Path {
        #[command(subcommand)]
        command: RegistryHostPathCommands,
    },
}

#[derive(Subcommand)]
enum RegistryHostPathCommands {
    /// List the preferred path and ordered fallbacks.
    List {
        host: String,
        /// Emit the path list as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Add or replace one connection path.
    Set {
        host: String,
        /// Path identifier (`primary`, `nebula`, `tailscale`, `lan`, ...).
        path: String,
        /// SSH destination ([user@]host[:port]) for this path.
        #[arg(long)]
        ssh: String,
        /// Fallback priority starting at 1; omitted preserves its position or appends.
        #[arg(long)]
        priority: Option<usize>,
        /// Emit the mutation receipt as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Remove one fallback connection path.
    Remove {
        host: String,
        /// Fallback path identifier.
        path: String,
        /// Emit the mutation receipt as JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum HostPrecheckRunnerCommands {
    /// Install or reconcile the isolated pre-check runner on TARGET.
    Install {
        target: String,
        /// Emit the lifecycle report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Read the installed runner service, identity and network boundary.
    Status {
        target: String,
        /// Emit the lifecycle report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Remove the runner, service definition and network boundary from TARGET.
    Remove {
        target: String,
        /// Emit the lifecycle report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Ensure one repository may schedule jobs on the managed runner group.
    RepositoryAdd {
        /// Repository name inside the wisent-ai organization.
        repository: String,
        /// Existing selected-repository runner group. Defaults to stado-precheck.
        #[arg(long)]
        runner_group: Option<String>,
        /// Emit the reconciliation report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Mint a dedicated Brama review bearer and install it as a repository secret.
    ModelReviewAdd {
        target: String,
        /// Repository name inside the wisent-ai organization.
        repository: String,
        /// Emit the reconciliation report as JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum HostPublisherRunnerCommands {
    /// Install or reconcile the desktop publisher and grant its release secrets.
    Install {
        target: String,
        /// Repository that may consume the shared release secrets. Repeat as needed.
        #[arg(long = "repository", required = true)]
        repositories: Vec<String>,
        /// Emit the lifecycle report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Grant one desktop repository the shared release secrets.
    RepositoryAdd {
        /// Repository name inside the wisent-ai organization.
        repository: String,
        /// Emit the reconciliation report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Create repository signing material and publish its required release secrets.
    Bootstrap {
        /// Repository name inside the wisent-ai organization.
        repository: String,
        /// Emit the bootstrap report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Issue or reuse the shared Developer ID certificate and grant it to repositories.
    DeveloperId {
        /// Registry host that runs the Account Holder Weles trajectory.
        target: String,
        /// Skarbiec item containing the Apple Account Holder credentials.
        #[arg(long)]
        account_item: String,
        /// Desktop repository that receives signing secrets. Repeat as needed.
        #[arg(long = "repository", required = true)]
        repositories: Vec<String>,
        /// Emit the bootstrap report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Read the installed runner service, identity and network boundary.
    Status {
        target: String,
        /// Emit the lifecycle report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Remove the runner, service definition and network boundary from TARGET.
    Remove {
        target: String,
        /// Emit the lifecycle report as JSON.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum HostCommands {
    /// Show the latest Stado health beacon and log tail for TARGET.
    Health {
        target: String,
        /// Emit the beacon and object metadata as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Publish one locally collected beacon through the scoped Stado health API.
    ///
    /// The `link` block (tailnet path, sleep/wake, interface changes) is
    /// collected here, on the host, and merged into the document about this
    /// machine before it is published.
    #[command(name = "publish-beacon")]
    PublishBeacon {
        /// JSON beacon file, or '-' for stdin.
        source: String,
        /// Print the document that would be published; publish nothing.
        #[arg(long)]
        print: bool,
    },
    /// Recover a registry-managed macOS host through its approved channel.
    Recover {
        target: String,
        /// Use the bundled registry snapshot when the canonical registry cannot be read.
        #[arg(long)]
        bundled_registry: bool,
        /// Replace Stado from an exact registry-trusted signed release before recovery.
        #[arg(long, value_name = "VERSION")]
        release: Option<String>,
    },
    /// Restore the core object API from its physical local store.
    #[command(name = "recover-object-api")]
    RecoverObjectApi {
        target: String,
        /// Emit the recovery report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Authorize TARGET's service resolver to read the registry from the
    /// service-directory authority.
    ///
    /// A resolver anywhere but on the authority host itself can only obtain a
    /// registry snapshot over ssh to that host, and a resolver with no snapshot
    /// binds none of its declared adapters — so the host publishes nothing at
    /// all, loudly in its log and invisibly everywhere else. This mints the
    /// resolver keypair on TARGET when it has none and appends its PUBLIC half
    /// to the authority account's authorized_keys, once. The private half is
    /// generated where it is used and never travels.
    #[command(name = "resolver-key")]
    ResolverKey {
        target: String,
        /// Emit the authorization report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Request a graceful reboot of TARGET through its approved channel.
    Reboot { target: String },
    /// Manage local macOS and Linux user accounts.
    #[command(subcommand)]
    User(HostUserCommands),
    /// Manage the isolated GitHub pre-check runner on a registry host.
    #[command(name = "precheck-runner", subcommand)]
    PrecheckRunner(HostPrecheckRunnerCommands),
    /// Manage the organization-wide GitHub desktop publisher on a registry host.
    #[command(name = "publisher-runner", subcommand)]
    PublisherRunner(HostPublisherRunnerCommands),
    /// Point TARGET's Weles recordings store at PATH.
    #[command(name = "weles-recordings-dir")]
    WelesRecordingsDir { target: String, path: String },
    /// Persist and immediately reconcile TARGET's NVIDIA board power cap.
    #[command(name = "gpu-power-limit")]
    GpuPowerLimit {
        target: String,
        watts: u32,
        /// Emit the registry generation and driver report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Publish TARGET's registry `weles` declaration as its placement policy.
    ///
    /// The worker decides what it may claim from a file on its own disk, not
    /// from the registry. This regenerates that file from the registry, stamps
    /// it with the generation it came from, and reports what changed.
    #[command(name = "publish-placement-policy")]
    PublishPlacementPolicy {
        target: String,
        /// Emit the publication and its action delta as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Manage the GUI-automation enablement of TARGET.
    #[command(name = "gui-automation", subcommand)]
    GuiAutomation(HostGuiAutomationCommands),
    /// Report or reclaim tagged build caches on TARGET.
    #[command(name = "build-caches", subcommand)]
    BuildCaches(HostBuildCacheCommands),
    /// Report TARGET's uptime, load averages and logged-in users.
    Uptime {
        target: String,
        /// Emit the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Check TARGET's ssh reachability AND health-beacon age as one verdict.
    Ping {
        target: String,
        /// Emit the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Report TARGET's disk usage and its registry cleanup policy state.
    Disk {
        target: String,
        /// Emit the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Move objects from one key prefix to another inside TARGET's store,
    /// on the host that holds it.
    ///
    /// The object API has GET, PUT, DELETE, list and stat and no move, so
    /// re-addressing an object used to mean pulling its body to the control
    /// plane and pushing it back. On 2026-08-30 doing that with 134 MiB GGUF
    /// parts took the always-on mac's release ingress down for ten minutes.
    /// Inside one store the bytes never move at all: the destination is
    /// hard-linked, hashed, compared against the source, and only then is the
    /// source unlinked.
    ///
    /// Previews by default. An existing destination is never overwritten.
    #[command(name = "object-relocate")]
    ObjectRelocate {
        target: String,
        /// Store namespace holding both addresses, e.g. probierz.
        #[arg(long)]
        namespace: String,
        /// The mis-addressed key prefix, e.g. ecosystem/probierz/.
        #[arg(long)]
        from_prefix: String,
        /// The key prefix it belongs under. Empty means the namespace root.
        #[arg(long, default_value = "")]
        to_prefix: String,
        /// Store root on the host. Defaults to the object API's own backing
        /// directory, $HOME/.stado/local-storage.
        #[arg(long)]
        store_root: Option<String>,
        /// Report what a pass would move and change nothing. The default, so
        /// it never has to be remembered.
        #[arg(long)]
        dry_run: bool,
        /// Relocate what the pass names.
        #[arg(long, conflicts_with = "dry_run")]
        apply: bool,
        /// Decide at most this many objects in one pass. 0 is every one of
        /// them; the command is resumable either way.
        #[arg(long, default_value_t = 0)]
        limit: usize,
        /// Emit the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Preview what the registry cleanup would delete on TARGET.
    Cleanup {
        target: String,
        /// Required: this command only ever previews.
        #[arg(long)]
        dry_run: bool,
        /// Emit the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Why HOST is claiming nothing: its own agent's published gates, the
    /// disk policy behind them, and what it declared against what it has.
    ///
    /// Read-only and safe against a live host. The Mac mini claimed nothing
    /// for hours at 2 GiB free against a 55 GiB policy, publishing
    /// `disk_pressure_unresolved` every tick, and no command said so.
    Gates {
        host: String,
        /// Emit the gates as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Why TARGET went quiet: beacon age, the path and endpoint it published,
    /// its last sleep and wake, its interface changes, the silences recorded
    /// against it, and what readers refused because of them.
    ///
    /// Read-only and safe against a live host. control-host was
    /// unreachable from 18:29 to 18:35 UTC on 2026-08-19 and came back on a
    /// direct path; nothing in this product carried a trace of it.
    Link {
        target: String,
        /// Emit the link report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Repair a stale, reachable host whose beacon publisher proves that the
    /// host-health API verifier is unavailable.
    ///
    /// Copies the authoritative route bearer into the object API authority's
    /// target-local verifier shadow, reconciles the existing least-privilege
    /// grant, waits for the normal publisher to write a newer beacon, and
    /// closes the recorded silence. Refuses every other diagnosis.
    #[command(name = "repair-link")]
    RepairLink {
        target: String,
        /// Emit the repair receipt as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Reclaim disk on HOST in declared stages, measuring each one.
    ///
    /// Previews by default: the host's own janitor pass, the release build
    /// scratch tree, and delivered product trees no `current` link and no
    /// live process references. `--apply` is the only thing that deletes and
    /// requires `--reason`, which is recorded on the host itself.
    Reclaim {
        host: String,
        /// Report what each stage would remove and delete nothing. The
        /// default, so it never has to be remembered.
        #[arg(long)]
        dry_run: bool,
        /// Remove what the stages name. Requires --reason.
        #[arg(long, conflicts_with = "dry_run")]
        apply: bool,
        /// Why the space is being reclaimed; appended to the host's own
        /// audit log beside the disk it changed.
        #[arg(long)]
        reason: Option<String>,
        /// Emit the staged report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Replace the tags of one Skarbiec item on TARGET, payload untouched.
    ///
    /// Consumers enumerate vault items by tag: Brama spends a subscription only
    /// when its item carries `brama:subscription` and `brama:agent:<agent>`, so
    /// an item that loses them leaves the fleet while its credential stays
    /// valid and every check that counts credentials keeps answering green.
    /// The owner key that may rewrite tags lives on the host, so this runs
    /// there, reads the item before and after, and reports both.
    #[command(name = "retag-vault-item")]
    RetagVaultItem {
        target: String,
        /// Vault item id, e.g. provider:kimi:brama-sub-wisent-app-kimi-primary.
        item: String,
        /// The complete tag list to store, comma separated. This replaces the
        /// item's tags rather than adding to them.
        ///
        /// Omit it to READ: the item's current state, revision and tags are
        /// reported and nothing is written. A command that can only replace a
        /// tag list forces an operator to guess the list they are replacing,
        /// and a guess that drops `brama:agent:<other>` silently unsubscribes
        /// another agent from a paid plan while every credential count stays
        /// green.
        #[arg(long)]
        tags: Option<String>,
        /// Emit the before/after report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Pull TARGET's Skarbiec mirror into its live vault without discarding
    /// local-only items.
    #[command(name = "sync-vault")]
    SyncVault {
        target: String,
        /// Emit the Skarbiec pull report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Store one typed item directly in TARGET's owner vault.
    ///
    /// The canonical JSON payload is read from stdin and carried only in the
    /// encrypted host channel's request body. Credential fields never enter a
    /// local or remote argument vector, and the host's other items are untouched.
    #[command(name = "vault-item-put")]
    VaultItemPut {
        target: String,
        /// Credential item id.
        item: String,
        /// Canonical Skarbiec item kind.
        #[arg(long = "type")]
        item_type: String,
        /// Emit the nonsecret before/after report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Mint one bounded Skarbiec bearer on TARGET's live vault.
    #[command(name = "vault-token-mint")]
    VaultTokenMint {
        target: String,
        consumer: String,
        /// Comma-separated Skarbiec capabilities.
        #[arg(long)]
        capabilities: String,
        /// Exact audience bound into the bearer.
        #[arg(long)]
        audience: String,
        /// Bearer lifetime in seconds.
        #[arg(long, default_value_t = 31_536_000)]
        ttl_seconds: u64,
        /// Replace an existing consumer's capability set.
        #[arg(long)]
        replace_capabilities: bool,
        /// Print only the bearer, for piping directly into a secret store.
        #[arg(long)]
        raw_token: bool,
        /// Emit nonsecret bearer metadata as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Make TARGET's dashboard verifier shadow and grant match every object
    /// namespace plus the route-scoped host-health bearer exactly.
    ///
    /// The route bearer is copied from the authoritative vault without
    /// rotating it. The verifier's existing bearer and expiry are preserved;
    /// stale capabilities are removed and missing reads are added.
    #[command(name = "reconcile-object-verifier")]
    ReconcileObjectVerifier {
        target: String,
        /// Emit the reconciled item set as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Ensure TARGET's release verifier can read one declared product publisher.
    ///
    /// The existing bearer, expiry, and unrelated product capabilities are
    /// preserved. Only PRODUCT's controller-owned shadow and read capability
    /// are reconciled, without printing either bearer.
    #[command(name = "reconcile-release-verifier")]
    ReconcileReleaseVerifier {
        target: String,
        /// Exact product key from release_api.publishers.
        #[arg(long)]
        product: String,
        /// Emit the reconciled item set as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Make TARGET's service-verifier grant match service_api.deployers exactly.
    ///
    /// The existing bearer and expiry are preserved. Stale capabilities are
    /// removed and missing read capabilities are added without printing the
    /// bearer or moving it through argv.
    #[command(name = "reconcile-service-verifier")]
    ReconcileServiceVerifier {
        target: String,
        /// Emit the reconciled item set as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Recover an audit-lock stall in Skarbiec and its loaded local dependants.
    #[command(name = "recover-skarbiec-audit")]
    RecoverSkarbiecAudit {
        target: String,
        #[arg(long)]
        json: bool,
    },
    /// Recover stale per-user GnuPG daemons blocking Skarbiec decryption.
    #[command(name = "recover-skarbiec-crypto")]
    RecoverSkarbiecCrypto {
        target: String,
        #[arg(long)]
        json: bool,
    },
    /// Repair Skarbiec acquisition state left by a different service user.
    #[command(name = "recover-skarbiec-acquisition-state")]
    RecoverSkarbiecAcquisitionState {
        target: String,
        #[arg(long)]
        json: bool,
    },
    /// The tail of one managed unit's own log on TARGET.
    ///
    /// A crash-looping unit says why in its log and nowhere else: the health
    /// beacon reports it failed and carries no log, and `host exec` is a
    /// read-only allowlist that cannot read a file.
    #[command(name = "unit-log")]
    UnitLog {
        target: String,
        /// Unit label as launchd knows it, e.g. com.wisent.always-on.brama.
        unit: String,
        /// Tail this many lines from each declared log path (default 40).
        #[arg(long)]
        lines: Option<u32>,
        #[arg(long)]
        json: bool,
    },
    /// Run Stado's native build and signed release journeys on TARGET.
    #[command(name = "verify-release-platform")]
    VerifyReleasePlatform {
        target: String,
        #[arg(long)]
        repo: String,
        #[arg(long = "ref")]
        revision: String,
        #[arg(long)]
        json: bool,
    },
    /// Classify HOST's local replica against the store it mirrors, object by
    /// object, and optionally reclaim the twins.
    ///
    /// Classifying deletes nothing. `--reclaim-twins --apply` deletes ONLY the
    /// replica objects that same pass proved byte-identical to the primary, by
    /// hashing both copies moments before the unlink — never a verdict an
    /// earlier run recorded, because an audit written to a file and a deletion
    /// run against it later is how a safety net becomes data loss.
    #[command(name = "backup-audit")]
    BackupAudit {
        target: String,
        /// Delete the twins this pass proves. Names them and deletes nothing
        /// without --apply.
        #[arg(long = "reclaim-twins")]
        reclaim_twins: bool,
        /// Actually delete what --reclaim-twins proved in this same pass.
        #[arg(long, requires = "reclaim_twins")]
        apply: bool,
        /// Emit the classification as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Open an encrypted reverse SSH forwarding channel to TARGET.
    #[command(name = "forward-local")]
    ForwardLocal {
        target: String,
        /// Safe name for the remote endpoint marker.
        name: String,
        /// Loopback port exposed on TARGET.
        #[arg(long)]
        remote_port: u16,
        /// Loopback port served by this control-plane host.
        #[arg(long)]
        local_port: u16,
        /// Emit the forwarding report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Close a forwarding channel opened by `forward-local` or
    /// `forward-remote`, and reconcile its markers.
    ///
    /// A tunnel the fleet can open and cannot close is a port it cannot
    /// reclaim: the detached `ssh -f -N` outlives the command that made it, and
    /// its marker under `~/.stado/forwards` keeps asserting an endpoint that
    /// may no longer carry anything. This ends the exact channel, deletes its
    /// markers, and re-reads the exposed port to confirm it stopped listening.
    ///
    /// The ssh process is matched on its complete `-R` or `-L` specification
    /// and its destination, never on the word `ssh`: this machine runs several
    /// forwards, and a match by program name would tear down the fleet's other
    /// channels.
    #[command(name = "forward-close")]
    ForwardClose {
        target: String,
        /// The forward name whose markers were written.
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Open an encrypted local SSH forwarding channel to TARGET.
    #[command(name = "forward-remote")]
    ForwardRemote {
        target: String,
        /// Safe name for the local endpoint marker.
        name: String,
        /// Loopback port served on TARGET.
        #[arg(long)]
        remote_port: u16,
        /// Loopback port exposed on this control-plane host.
        #[arg(long)]
        local_port: u16,
        /// Emit the forwarding report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Declare the exact version TARGET must run for one managed binary.
    #[command(name = "declare-version")]
    DeclareVersion {
        target: String,
        #[arg(long)]
        binary: String,
        #[arg(long)]
        version: String,
        #[arg(long)]
        json: bool,
    },
    /// Promote one exact published version to every registry target.
    #[command(name = "promote-version")]
    PromoteVersion {
        #[arg(long)]
        binary: String,
        #[arg(long)]
        version: String,
        #[arg(long)]
        json: bool,
    },

    /// Compare active versions with desired state; omit TARGET for the fleet.
    Reconcile {
        target: Option<String>,
        /// Close every deliverable difference.
        #[arg(long)]
        apply: bool,
        #[arg(long)]
        json: bool,
    },
    /// Which Skarbiec vaults the fleet holds; omit TARGET to ask every host.
    Vaults {
        /// Ask one host instead of the whole registry.
        target: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Run one approved command on TARGET (allowlist, not a shell). Every
    /// entry is read-only except the declared provider sign-in repairs.
    Exec {
        target: String,
        /// Emit the report as JSON instead of the host's raw output.
        #[arg(long)]
        json: bool,
        /// The approved command, after `--`. Run with an unapproved one to
        /// see the allowlist.
        #[arg(last = true)]
        command: Vec<String>,
    },
    /// Place an interactive Jeden RPC session on a live registry host and
    /// attach it to this process's stdin/stdout.
    #[command(name = "jeden-connect")]
    JedenConnect {
        /// Repository name under ~/Documents/CodingProjects/Wisent.
        workspace: String,
        /// Reconnect to the host that owns an existing durable session.
        #[arg(long)]
        target: Option<String>,
        /// Require the selected host to own this ~/.jeden/sessions ledger.
        #[arg(long)]
        resume: Option<String>,
    },
    /// Remove one file from TARGET's home, with guards a bare `rm` over ssh
    /// does not have: the path must live under a managed area of the approved
    /// account's home, be a regular file owned by that account, and never be
    /// a symlink — anything else is refused before anything is deleted.
    #[command(name = "remove-file")]
    RemoveFile {
        target: String,
        /// Absolute path on the target. Only `$HOME/Library/LaunchAgents`
        /// and `$HOME/.stado` are deletable by this command; a system path is
        /// refused with the privileged command that could remove it named.
        path: String,
        /// Emit the removal report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Read TARGET's crontab, and optionally prune one entry from it.
    ///
    /// The periodic table is the one place a fleet host can declare a
    /// process that no launchd domain and no registry document mentions.
    /// charless-mac-mini carried four `@reboot` entries outside both, two of
    /// which restart duplicates that had just been retired with verified
    /// postconditions — so every repair on that host was one reboot from
    /// coming back, and the only way to change the table was a bare
    /// `crontab -e` over ssh.
    ///
    /// `--prune` previews by default and refuses anything but a single
    /// matching line that references `$HOME/.stado`; `--apply` saves the
    /// whole table under `$HOME/.stado/cron-backups` first and prints the
    /// `--restore` command that puts it back.
    Cron {
        target: String,
        /// Literal text naming the ONE entry to remove; usually the script's
        /// path. Refused when it reaches more than one line.
        #[arg(long)]
        prune: Option<String>,
        /// Install a table saved by an earlier `--prune --apply`.
        #[arg(long, conflicts_with = "prune")]
        restore: Option<String>,
        /// Change the table. Without it, `--prune` only reports what it would
        /// remove.
        #[arg(long)]
        apply: bool,
        /// Emit the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Deliver the checked-in Skarbiec acquisition-scope catalog to TARGET and
    /// register it against the host's fleet vault, then print the reconciled
    /// status.
    #[command(name = "sync-acquisition-scopes")]
    SyncAcquisitionScopes {
        target: String,
        /// Local acquisition-scope catalog file to deliver and register.
        source: String,
    },
    /// Report TARGET's stado-managed binaries, forward markers and loopback
    /// listeners, and whether each marker still matches a live listener.
    Inventory {
        target: String,
        /// Emit the inventory and its reconciliation as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Report every program TARGET actually runs with its version, digest and
    /// whether it came out of a release; omit TARGET to read what every host
    /// has already reported. A host that has never reported is a failure
    /// wherever the report is judged, never a pass.
    Software {
        target: Option<String>,
        /// Emit the report and its findings as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Report which commit produced each artifact TARGET carries, and whether
    /// that commit is reachable from origin/main. An artifact with no manifest
    /// is reported unprovenanced, never omitted.
    Provenance {
        target: String,
        /// Emit the manifests and their reachability as JSON.
        #[arg(long)]
        json: bool,
    },
    /// What TARGET's Weles worker is doing: its staged and installed releases,
    /// whether its worker API answers, and its newest recorded runs with each
    /// run's own verdict. Counts and timestamps only; recordings stay on the
    /// host.
    #[command(name = "weles-activity")]
    WelesActivity {
        target: String,
        /// Emit the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Read the authenticated artifact inventory, or one exact artifact, from
    /// a completed Weles browser run on TARGET.
    #[command(name = "weles-run-diagnostics")]
    WelesRunDiagnostics {
        target: String,
        /// Weles run identifier returned by the browser task.
        run_id: String,
        /// Exact artifact path from the run inventory.
        #[arg(long)]
        file: Option<String>,
        /// Emit the report as JSON. Binary file content is base64 encoded.
        #[arg(long)]
        json: bool,
    },
    /// Inspect every image rendered by one HTTPS surface in a read-only Weles
    /// browser session on TARGET. The objective and safety constraints are
    /// fixed by Stado; the caller supplies only the URL.
    #[command(name = "weles-image-inspect")]
    WelesImageInspect {
        target: String,
        /// HTTPS page Weles must render and inspect.
        #[arg(long)]
        url: String,
        /// Emit the complete redacted Weles result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Enqueue one batch of `generic_capture` actions on TARGET's Weles
    /// admission API from a checked-in capture plan. The plan is refused in
    /// full before the host is contacted, the loopback API is reached over the
    /// registry's own encrypted SSH channel for the length of the command, and
    /// every artifact lands in Stado storage under the plan's own prefixes.
    #[command(name = "weles-capture")]
    WelesCapture {
        target: String,
        /// Capture plan file, schema `wisent.weles-capture-plan.v1`.
        #[arg(long)]
        plan: String,
        /// Use this batch id instead of the one the plan declares.
        #[arg(long)]
        batch: Option<String>,
        /// Emit the enqueue report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Per-action state of one capture batch — queued, running, done or
    /// failed — plus the artifact keys already present in Stado storage under
    /// the batch prefix. Read-only. Retrieval is `stado storage get`.
    #[command(name = "weles-capture-status")]
    WelesCaptureStatus {
        target: String,
        /// Batch id to report on.
        #[arg(long)]
        batch: String,
        /// Emit the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Read, or declare, one Skarbiec capability route ON TARGET.
    ///
    /// A capability route maps a resource the broker is asked to resolve —
    /// `origin:<page origin>/<field class>` for a browser fill — onto one
    /// vault item and field. The table decides which credential a login form
    /// receives, and it is per-host: charless-mac-mini holds its own, so a
    /// route declared on an operator's laptop makes nothing resolvable there.
    /// `capability-issue` refuses a resource with no route at issue time,
    /// which is why this exists as its own verb rather than as a side effect
    /// of some flow that needed one.
    ///
    /// Without `--resource` this is a READ: every route on TARGET with that
    /// host's own answer for it — whether the item is one it can open and
    /// whether the field is one that item carries. With all four flags it
    /// declares one route. Skarbiec keeps the previous table beside the new
    /// one, records the reason in its journal, reports an identical route as
    /// unchanged, and refuses to repoint a live route.
    #[command(name = "capability-route")]
    CapabilityRoute {
        target: String,
        /// The resource to map, e.g. `origin:https://accounts.google.com/email`.
        #[arg(long)]
        resource: Option<String>,
        /// The vault item on TARGET that holds the credential.
        #[arg(long)]
        item: Option<String>,
        /// The field of that item, e.g. `username` or `password`.
        #[arg(long)]
        field: Option<String>,
        /// Why this route exists. Required by Skarbiec for a declaration, and
        /// carried into its journal beside the table: a change to which
        /// credential a form receives is never self-explanatory later.
        #[arg(long)]
        reason: Option<String>,
        /// Ask TARGET to verify its whole table and report the SENTENCE behind
        /// every route that cannot deliver, instead of the two booleans the
        /// listing prints. A non-interactive channel may be unable to open a
        /// vault the broker service on that host opens fine, and only the
        /// sentence tells those apart.
        #[arg(long)]
        verify: bool,
        /// Address one broker instance's capability state instead of the
        /// host's vault-adjacent default. `capability-serve` is started with
        /// whatever its launcher exports, and only that instance can redeem
        /// what is issued into it. A leading `$HOME/` expands on the host.
        #[arg(long)]
        capability_file: Option<String>,
        /// The route table that same instance resolves against.
        #[arg(long)]
        routes_file: Option<String>,
        /// Emit the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Run one browser task on TARGET's Weles worker and report its result.
    ///
    /// The general submission surface. `weles-capture` hard-codes
    /// `generic_capture`, which charless-mac-mini's worker does not accept, and
    /// `weles-image-inspect` submits the allowlisted `generic_browser_task`
    /// with its objective and constraints fixed in product code. So the only
    /// action that host would run was reachable only through a command that
    /// could not be told what to do, and every browser workflow this fleet
    /// owns sat behind that.
    ///
    /// The action name is checked against TARGET's own
    /// `WELES_ACTION_ALLOWLIST` before any channel is opened, and the
    /// allowlist is read byte-exact rather than through `env-show`, which
    /// clamps values at 400 characters and would silently truncate a
    /// 4488-character list to its first 25 entries. An action the worker would
    /// refuse is refused here, naming the action and the host.
    ///
    /// The request is held open for the run, so this reports what the run
    /// produced rather than a queue receipt.
    /// Activate a release already staged on TARGET by running THAT release's
    /// own installer, once.
    ///
    /// A managed host installs its own releases with the installer inside its
    /// active release. When that copy is broken the host cannot install the
    /// release that repairs it - the repair is staged, verified and
    /// unreachable, and every delivery after it piles up behind the same
    /// unparseable script. This runs the staged copy instead. Same env file,
    /// same digest contract, same script; only which copy executes differs.
    ///
    /// Refuses if the staged archive does not hash to the digest the
    /// deployment env file declares, and refuses to run an installer that does
    /// not parse. Reports the API's state either side, and fails if a port
    /// that was answering before is silent after.
    #[command(name = "activate-staged-release")]
    ActivateStagedRelease {
        /// Registry host holding the staged release.
        target: String,
        /// Product whose coordinate the env file declares, e.g. weles-worker.
        #[arg(long, default_value = "weles-worker")]
        product: String,
        /// Deployment env file naming the release coordinate.
        #[arg(long, default_value = "$HOME/.config/weles/worker.env")]
        env_file: String,
        /// Loopback port whose liveness is checked either side of the run.
        #[arg(long, default_value_t = 8788)]
        port: u16,
        /// Emit the report as JSON.
        #[arg(long)]
        json: bool,
    },
    #[command(name = "weles-browser-task")]
    WelesBrowserTask {
        target: String,
        /// Page the task starts on.
        #[arg(long)]
        url: String,
        /// What the agent must accomplish. `@path` reads the objective from a
        /// file, for the long ones that do not belong in a shell history.
        #[arg(long)]
        objective: String,
        /// Stable recording label. `--fresh-profile` controls profile identity.
        #[arg(long)]
        session_label: String,
        /// Action to run; must be one TARGET's allowlist carries.
        #[arg(long, default_value = crate::deploy::weles_browser_task::DEFAULT_ACTION)]
        action: String,
        /// Exact action catalog shipped by the active Weles release.
        #[arg(long, default_value = crate::deploy::weles_browser_task::DEFAULT_ALLOWLIST_FILE)]
        allowlist_file: String,
        /// Exact Weles login item for a named credential trajectory.
        #[arg(long)]
        login_item: Option<String>,
        /// Bind this run to one account identity, which is what keys the
        /// browser profile. Weles hashes it into a profile directory and the
        /// API puts it in the trajectory's `ACCOUNT_ID`, so two runs sharing it
        /// share cookies and a signed-in session. Without it every run is a
        /// brand-new device to the site being driven, which is how a sign-in
        /// that succeeded once cannot be built on and why each attempt draws a
        /// first-visit risk check.
        #[arg(long)]
        account_id: Option<String>,
        /// Give the run a new account identity, which makes Weles create a new
        /// browser profile directory instead of clearing or reusing one.
        #[arg(long)]
        fresh_profile: bool,
        /// Carry "this run may sign in" into the agent's instructions. This is
        /// a HINT, not an enforced restriction: Weles appends the
        /// read_only/no_login/no_mutation constraints to the model's goal text
        /// and checks them nowhere, and the agent holds fill, click, navigate
        /// and store_credential whether or not this is set. Its one mechanical
        /// effect is that --sign-in-origin is refused without it.
        #[arg(long)]
        allow_login: bool,
        /// Sign in on this page origin with the account Skarbiec holds, e.g.
        /// `https://accounts.google.com`. Stado mints one single-use,
        /// one-hour `weles.browser.fill` capability per field — email and
        /// password — under one authorization id, and sends only those
        /// references: no secret enters argv, the objective, a log line or the
        /// report. The worker redeems each against its own broker at fill time
        /// and zeroes the plaintext. Requires --sign-in-item and
        /// --allow-login. A bare origin only: Weles compares it against the
        /// live page's own origin, so a run that redirects elsewhere before the
        /// prefill is refused there rather than filled.
        #[arg(long)]
        sign_in_origin: Option<String>,
        /// The vault item holding that account. Checked against Skarbiec's
        /// capability route table before anything is minted: the item a route
        /// names is the item that would be read, and a disagreement is refused
        /// rather than silently resolved in the route's favour.
        #[arg(long)]
        sign_in_item: Option<String>,
        /// Hand every sign-in capability to the agent instead of prefilling the
        /// first one. For hosts whose installed runtime fills at page load
        /// without waiting for the field: weles before 0.5.41 spends a
        /// capability whether or not the input has rendered, so a slow
        /// identifier page silently costs the fill and cannot be retried. The
        /// agent redeems each reference on the page that has the field.
        #[arg(long)]
        defer_fills: bool,
        /// Prefill every sign-in capability on the first loaded page. Use this
        /// for forms that render the identifier and password together. Weles
        /// 0.5.41 and newer leave a capability unspent when its field is absent,
        /// so a later agent step can still redeem it. Mutually exclusive with
        /// --defer-fills and requires --sign-in-origin.
        #[arg(long, conflicts_with = "defer_fills", requires = "sign_in_origin")]
        prefill_all: bool,
        /// The saved-trajectory key. Defaults to the session label, which is
        /// also the browser profile's name - two different things that only
        /// look alike. A run whose `done` carried an error is still codified
        /// under that key and replayed verbatim on the next run, so resuming a
        /// profile means inheriting the failure that last used it unless this
        /// names a fresh flow.
        #[arg(long)]
        flow_name: Option<String>,
        /// Run with a visible window. Some sign-in flows refuse headless.
        #[arg(long)]
        windowed: bool,
        /// Emit the complete result as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Verify, and with --repair complete, the browser runtime TARGET's Weles
    /// release declares it needs.
    ///
    /// The worker records its sessions, so a missing recording dependency kills
    /// `browserContext.newPage` before any navigation and every browser task on
    /// the host fails. On charless-mac-mini that was Playwright's ffmpeg, absent
    /// at ms-playwright/ffmpeg-1011/ffmpeg-mac, and three runs had already
    /// failed that way before anyone looked. Recording is the evidence Weles
    /// exists to keep, so the repair completes the runtime rather than turning
    /// recording off.
    ///
    /// The requirement is read from `browsers.json` inside the installed
    /// release, never hardcoded here, because Playwright pins an exact revision
    /// per component and a constant would verify the wrong path the moment the
    /// release moved. Verification reports every component the release installs
    /// by default; --repair installs only the ones named, defaulting to the one
    /// Weles takes from that cache, so a repair never downloads browsers
    /// nothing drives.
    #[command(name = "weles-browser-runtime")]
    WelesBrowserRuntime {
        target: String,
        /// Component to install with --repair; repeat for each. Defaults to
        /// ffmpeg, which is what the recording path needs.
        #[arg(long = "component")]
        components: Vec<String>,
        /// Install the missing components, then verify again.
        #[arg(long)]
        repair: bool,
        #[arg(long)]
        json: bool,
    },
    /// Read TARGET's effective Stado configuration through its fleet channel.
    ConfigShow { target: String },
    /// Persist one dotted Stado configuration value on TARGET.
    ConfigSet {
        target: String,
        key: String,
        /// JSON value, or a bare string as accepted by `stado config set`.
        value: String,
        /// Reconcile this registry-managed service after the atomic write so
        /// long-lived processes observe the new configuration immediately.
        #[arg(long)]
        reload_service: Option<String>,
    },
    /// Deliver one registry-declared managed binary to TARGET.
    Release {
        target: String,
        #[arg(long)]
        binary: String,
        #[arg(long)]
        version: String,
        /// Report the plan without mutation.
        #[arg(long)]
        dry_run: bool,
        /// Reinstall the exact immutable bytes even when the host reports the
        /// same semantic version; used to replace an unmanaged same-version file.
        #[arg(long)]
        reinstall: bool,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum HostBuildCacheCommands {
    /// List tagged cache directories older than --min-age-days with sizes.
    Report {
        target: String,
        /// Absolute directory to search.
        #[arg(long)]
        root: String,
        /// Only consider directories untouched for this many whole days.
        #[arg(long)]
        min_age_days: String,
    },
    /// Delete those directories.
    Prune {
        target: String,
        #[arg(long)]
        root: String,
        #[arg(long)]
        min_age_days: String,
        /// Remove tagged caches even when they were used today.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum HostGuiAutomationCommands {
    /// Report autologin, remote management, TCC and automation artifacts.
    Status { target: String },
    /// Configure persistent GUI login, install CuaDriver, and grant Accessibility.
    Enable { target: String },
    /// Grant the installed CuaDriver app Accessibility for the host's GUI user.
    #[command(name = "grant-accessibility")]
    GrantAccessibility { target: String },
    /// Revert the enablement: autologin, kcpassword, remote management,
    /// the driver's accessibility grant, and the installed artifacts.
    Disable {
        target: String,
        /// Bundle id whose accessibility grant is revoked; omitted leaves TCC alone.
        #[arg(long)]
        bundle: Option<String>,
    },
}

#[derive(Subcommand)]
enum HostUserCommands {
    /// Create USERNAME on selected registry-managed hosts over SSH.
    Create {
        username: String,
        /// Registry target name. Repeat to provision several hosts.
        #[arg(long)]
        target: Vec<String>,
        /// Provision every kind=local registry target with SSH configured.
        #[arg(long)]
        all: bool,
        /// Account display name.
        #[arg(long)]
        full_name: Option<String>,
        /// Absolute login shell; host OS default if omitted.
        #[arg(long, default_value = "")]
        shell: String,
        /// Create an administrator account instead of a standard user.
        #[arg(long)]
        admin: bool,
        /// Require the new user to change the initial password on first login.
        #[arg(long)]
        require_password_change: bool,
        /// Validate and list targets without connecting.
        #[arg(long)]
        dry_run: bool,
        #[arg(long, default_value = "gcs", value_parser = ["gcs", "local", "auto"])]
        registry_source: String,
    },
    /// Delete USERNAME from a registry-managed host over SSH.
    Delete {
        username: String,
        /// Registry target name.
        #[arg(long)]
        target: String,
        /// Leave the home directory in place.
        #[arg(long)]
        keep_home: bool,
    },
}

#[derive(Subcommand)]
enum VastCommands {
    /// List the configured Vast.ai machine on the marketplace.
    /// Requires stado-vast/api_key in Skarbiec and WC_VAST_MACHINE_ID unless
    /// the machine can be discovered automatically.
    List {
        /// Per-GPU-hour rental price USD (default 0.50).
        #[arg(long, default_value_t = 0.50)]
        price_gpu: f64,
        /// Per-GB-month disk price USD (default 0.05).
        #[arg(long, default_value_t = 0.05)]
        price_disk: f64,
        /// Optional minimum interruptible-bid price floor.
        #[arg(long)]
        price_min_bid: Option<f64>,
    },
    /// Remove every offer for our Vast.ai machine, blocking new renters.
    /// Existing rentals are not terminated. Requires stado-vast/api_key in
    /// Skarbiec and a resolvable machine id.
    Unlist,
    /// Show Vast.ai's current view of our machine (rentals, listed).
    Status,
    /// One-shot snapshot of the Vast bridge + wisent-compute state.
    Monitor {
        /// Logical Stado queue namespace (default wisent-compute).
        #[arg(long, default_value = "wisent-compute")]
        bucket: String,
    },
    /// Daemon: list on Vast.ai when wisent-compute is idle, unlist when work appears.
    #[command(name = "auto-list")]
    AutoList {
        /// Wisent-compute must be idle this many seconds before listing.
        #[arg(long, default_value_t = 300)]
        idle_window_s: i64,
        /// Polling interval against configured Stado queue storage.
        #[arg(long, default_value_t = 10)]
        poll_interval_s: i64,
        /// Per-GPU-hour rental price USD when we list.
        #[arg(long, default_value_t = 0.50)]
        price_gpu: f64,
        /// Cap the max rental length any Vast renter can buy from
        /// this offer (default 3600s = 1h). 0 to leave open-ended.
        #[arg(long, default_value_t = 3600)]
        max_duration_s: i64,
        /// Print the toggle decisions without calling the Vast API.
        #[arg(long)]
        dry_run: bool,
    },
}

/// Parse argv and run the dispatched command, then present whatever went
/// wrong to the operator.
///
/// A failure leaves three things behind, in this order: the command's own
/// `Error: {msg}` line, unchanged and unabridged; one classified sentence
/// saying whether this is our outage or their request and whether a retry
/// can help; and one structured log line for whatever ships this host's
/// stderr. There is no fourth thing — in particular no HTTP call to an
/// analytics collector, which on a failure path is just one more dependency
/// that can hang the tool.
///
/// Exit codes: 0 on success, [`CLICK_ERROR_CODE`] on a runtime error, 2 on
/// usage errors (clap parse failures exit 2 on their own) and for
/// not-yet-implemented commands, and [`crate::failure::retry_exit_code`]
/// when the failure is one a retry can clear. See `docs/cli.md`.
pub async fn main_entry() -> i32 {
    // Parse in two steps rather than through `Cli::parse()` — which is
    // exactly these two steps — so the matches tree is still in hand
    // afterwards. It is the only place the subcommand path exists as data
    // rather than as a match arm, and that path is the failure point.
    let matches = Cli::command().get_matches();
    let point = failure_point(&matches);
    let service = failure_service(&matches);
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(err) => err.exit(),
    };
    match dispatch(cli).await {
        // Success.
        Ok(()) => i32::default(),
        Err(err) => {
            // A silent failure already printed its own diagnosis; adding a
            // classification line here would contradict a command that
            // deliberately said nothing.
            let Some(message) = err.message.as_deref() else {
                return err.code;
            };
            let code = crate::failure::classify_message(message);
            eprintln!("Error: {message}");
            eprintln!("{}", crate::failure::operator_line(code));
            crate::failure::log_failure(&point, service, code, message);
            // Usage errors keep their own code: no amount of retrying fixes
            // an argument, whatever the message happens to read like.
            if err.code == CLICK_ERROR_CODE {
                code.exit_code(err.code)
            } else {
                err.code
            }
        }
    }
}

/// The dotted id of the command that failed, built from the declared
/// subcommand names only — `cli.host.user.create`, never an argument value,
/// so the field stays a low-cardinality key a log query can group by.
fn failure_point(matches: &clap::ArgMatches) -> String {
    let mut point = String::from("cli");
    let mut node = matches;
    while let Some((name, sub)) = node.subcommand() {
        point.push('.');
        point.push_str(name);
        node = sub;
    }
    point
}

/// The dependency axis an operator reasons about, coarser than the command
/// tree: when the queue's storage is down, `submit`, `status` and `storage ls`
/// are one incident, not three.
fn failure_service(matches: &clap::ArgMatches) -> &'static str {
    match matches.subcommand_name().unwrap_or_default() {
        "submit" | "status" | "cancel" | "results" | "job" | "machine" | "queue" | "storage"
        | "artifact" => "queue",
        "fleet"
        | "host"
        | "registry"
        | "builds"
        | "service"
        | "instances"
        | "resources"
        | "recovery"
        | "bootstrap"
        | "doctor"
        | "disk-cleanup"
        | "install-disk-cleanup" => "fleet",
        "secrets" => "credentials",
        "billing" | "cost" | "quota" => "billing",
        "mail" => "mail",
        "azure" | "cloudflare" | "vast" | "blast-radius" => "provider",
        "coordinator"
        | "resolver"
        | "release"
        | "database"
        | "schedule"
        | "agent"
        | "local-control-plane"
        | "cloud-control-plane" => "control-plane",
        _ => "stado",
    }
}

async fn dispatch(cli: Cli) -> Result<(), CmdError> {
    let catalog_problems = crate::capabilities::validate_catalog();
    if !catalog_problems.is_empty() {
        return Err(CmdError::click(format!(
            "capability catalog is invalid: {}",
            catalog_problems.join("; ")
        )));
    }
    let Some(command) = cli.command else {
        print_onboarding();
        return Ok(());
    };
    match command {
        Commands::PackageRoot => {
            // Python prints the installed package source root; the Rust
            // equivalent is the crate data directory (profiles, templates,
            // registry) used by desktop provisioning.
            println!("{}", crate::data_dir().display());
            Ok(())
        }
        Commands::Capabilities { capability, json } => {
            capabilities::run(capability.as_deref(), json)
        }
        Commands::Overview { json } => overview::run(json).await,
        Commands::BlastRadius(args) => blast_radius::run(&args).await,
        Commands::Resources(command) => resources::dispatch(command).await,
        Commands::Optimize(command) => autonomy_cmd::dispatch_optimize(command).await,
        Commands::Billing(sub) => billing::dispatch(&sub).await,
        Commands::Azure(sub) => azure::dispatch(sub).await,
        Commands::Cloudflare(sub) => cloudflare::dispatch(sub).await,
        Commands::Mail(sub) => mail::dispatch(&sub).await,
        Commands::Submit(args) => submit::run(&args).await,
        Commands::Status { filter_id } => status::run(filter_id.as_deref()).await,
        Commands::Cancel { job_id, terminate } => cancel::run(&job_id, terminate).await,
        Commands::Job(sub) => job::dispatch(sub).await,
        Commands::Results { job_id, output_dir } => results::run(&job_id, &output_dir).await,
        Commands::Profiles { name } => profiles_cmd::run(name.as_deref()),
        Commands::Config { sub, key, value } => {
            config_cmd::run(&sub, key.as_deref(), value.as_deref())
        }
        Commands::Machine(sub) => match sub {
            MachineCommands::Submit { request_file } => machine::submit(&request_file).await,
            MachineCommands::Status { job_id } => machine::status(&job_id).await,
            MachineCommands::Logs {
                job_id,
                cursor,
                limit,
            } => machine::logs(&job_id, cursor, limit).await,
            MachineCommands::Cancel { job_id } => machine::cancel(&job_id).await,
            MachineCommands::Artifacts { job_id, output_dir } => {
                machine::artifacts(&job_id, &output_dir).await
            }
        },
        Commands::Agent {
            gpu_type,
            target,
            auto,
            idle_shutdown,
            kind,
            vast_auto_list,
            vast_price_gpu,
            vast_max_duration_s,
        } => {
            agent::run(
                gpu_type,
                target,
                auto,
                idle_shutdown,
                kind,
                vast_auto_list,
                vast_price_gpu,
                vast_max_duration_s,
            )
            .await
        }
        Commands::Schedule(sub) => match sub {
            ScheduleCommands::Create(args) => schedule::create(&args).await,
            ScheduleCommands::List => schedule::list().await,
            ScheduleCommands::Show { schedule_id } => schedule::show(&schedule_id).await,
            ScheduleCommands::Rm { schedule_id } => schedule::rm(&schedule_id).await,
            ScheduleCommands::Pause { schedule_id } => schedule::pause(&schedule_id).await,
            ScheduleCommands::Resume { schedule_id } => schedule::resume(&schedule_id).await,
            ScheduleCommands::Run { schedule_id } => schedule::run(&schedule_id).await,
        },
        Commands::DiskCleanup {
            once,
            watch,
            dry_run,
        } => disk_cleanup::run(once, watch, dry_run).await,
        Commands::InstallDiskCleanup => disk_cleanup::install().await,
        Commands::Artifact(sub) => artifact::dispatch(sub).await,
        Commands::Release(sub) => release_cmd::dispatch(sub).await,
        Commands::Cost(sub) => cost::dispatch(&sub).await,
        Commands::Vast(sub) => vast::dispatch(&sub).await,
        Commands::Quota { json, sub } => quota::dispatch(json, &sub).await,
        Commands::Coordinator { target, once } => coordinator::run(target, once).await,
        Commands::Dashboard {
            bind,
            port,
            enrollment_only,
        } => dashboard::run(bind, port, enrollment_only).await,
        Commands::LocalControlPlane {
            bind,
            port,
            interval,
        } => control_plane::local(bind, port, interval).await,
        Commands::CloudControlPlane {
            bind,
            port,
            interval,
        } => control_plane::cloud(bind, port, interval).await,
        Commands::Registry(sub) => match sub {
            RegistryCommands::Validate { path } => registry::validate(path),
            RegistryCommands::Push {
                path,
                force,
                allow_empty_fleet,
            } => registry::push(path, force, allow_empty_fleet).await,
            RegistryCommands::Pull => registry::pull().await,
            RegistryCommands::SelfTarget { name_only } => registry::self_target(name_only).await,
            RegistryCommands::Doctor { json } => registry::doctor(json).await,
            RegistryCommands::Host(command) => match command {
                RegistryHostCommands::Add {
                    host,
                    ssh,
                    kind,
                    release_platform,
                } => registry::host_add(&host, &ssh, &kind, &release_platform).await,
                RegistryHostCommands::Path { command } => match command {
                    RegistryHostPathCommands::List { host, json } => {
                        registry::host_path_list(&host, json).await
                    }
                    RegistryHostPathCommands::Set {
                        host,
                        path,
                        ssh,
                        priority,
                        json,
                    } => registry::host_path_set(&host, &path, &ssh, priority, json).await,
                    RegistryHostPathCommands::Remove { host, path, json } => {
                        registry::host_path_remove(&host, &path, json).await
                    }
                },
            },
            RegistryCommands::BeaconAge { json } => registry::beacon_age(json).await,
        },
        Commands::Builds(sub) => builds::run(sub).await,
        Commands::Fleet(sub) => fleet::run(sub).await,
        Commands::Identity(sub) => match sub {
            IdentityCommands::List { json } => identity::list(json).await,
            IdentityCommands::Verify {
                kind,
                identity,
                json,
            } => identity::verify(kind, identity, json).await,
        },
        Commands::Host(sub) => match sub {
            HostCommands::Health { target, json } => host::health(&target, json).await,
            HostCommands::PublishBeacon { source, print } => {
                host::publish_beacon(&source, print).await
            }
            HostCommands::Recover {
                target,
                bundled_registry,
                release,
            } => host::recover(&target, bundled_registry, release.as_deref()).await,
            HostCommands::RecoverObjectApi { target, json } => {
                host::recover_object_api(&target, json).await
            }
            HostCommands::ResolverKey { target, json } => {
                host::authorize_resolver_key(&target, json).await
            }
            HostCommands::Reboot { target } => host::reboot(&target).await,
            HostCommands::User(HostUserCommands::Create {
                username,
                target,
                all,
                full_name,
                shell,
                admin,
                require_password_change,
                dry_run,
                registry_source,
            }) => {
                host::user_create(
                    &username,
                    target,
                    all,
                    full_name,
                    shell,
                    admin,
                    require_password_change,
                    dry_run,
                    &registry_source,
                )
                .await
            }
            HostCommands::User(HostUserCommands::Delete {
                username,
                target,
                keep_home,
            }) => host::user_delete(&username, &target, keep_home).await,
            HostCommands::WelesRecordingsDir { target, path } => {
                host::weles_recordings_dir(&target, &path).await
            }
            HostCommands::GpuPowerLimit {
                target,
                watts,
                json,
            } => host::gpu_power_limit(&target, watts, json).await,
            HostCommands::PublishPlacementPolicy { target, json } => {
                placement::publish_placement_policy(&target, json).await
            }
            HostCommands::GuiAutomation(HostGuiAutomationCommands::Status { target }) => {
                host::gui_automation_status(&target).await
            }
            HostCommands::GuiAutomation(HostGuiAutomationCommands::Enable { target }) => {
                host::gui_automation_enable(&target).await
            }
            HostCommands::GuiAutomation(HostGuiAutomationCommands::GrantAccessibility {
                target,
            }) => host::gui_automation_grant_accessibility(&target).await,
            HostCommands::GuiAutomation(HostGuiAutomationCommands::Disable { target, bundle }) => {
                host::gui_automation_disable(&target, bundle.as_deref().unwrap_or("")).await
            }
            HostCommands::BuildCaches(HostBuildCacheCommands::Report {
                target,
                root,
                min_age_days,
            }) => host::build_caches(&target, &root, &min_age_days, false, false).await,
            HostCommands::BuildCaches(HostBuildCacheCommands::Prune {
                target,
                root,
                min_age_days,
                force,
            }) => host::build_caches(&target, &root, &min_age_days, true, force).await,
            HostCommands::Uptime { target, json } => host::uptime(&target, json).await,
            HostCommands::Ping { target, json } => host::ping(&target, json).await,
            HostCommands::Disk { target, json } => host::disk(&target, json).await,
            // Same default as `host reclaim`: `--dry-run` is what happens
            // when nothing is asked for, and clap refuses it beside `--apply`.
            HostCommands::ObjectRelocate {
                target,
                namespace,
                from_prefix,
                to_prefix,
                store_root,
                dry_run: _,
                apply,
                limit,
                json,
            } => {
                let plan = crate::deploy::host_object_relocate::RelocatePlan {
                    namespace,
                    from: from_prefix,
                    to: to_prefix,
                    store_root,
                    apply,
                    limit,
                };
                host::object_relocate(&target, &plan, json).await
            }
            HostCommands::Cleanup {
                target,
                dry_run,
                json,
            } => host::cleanup(&target, dry_run, json).await,
            HostCommands::Gates { host: target, json } => host::gates(&target, json).await,
            HostCommands::Link { target, json } => host::link(&target, json).await,
            HostCommands::RepairLink { target, json } => host::repair_link(&target, json).await,
            // `--dry-run` is the default and needs no argument: `--apply` is
            // the only flag that changes anything, and clap already refuses
            // the two together.
            HostCommands::Reclaim {
                host: target,
                dry_run: _,
                apply,
                reason,
                json,
            } => host::reclaim(&target, apply, reason.as_deref(), json).await,
            HostCommands::PrecheckRunner(command) => match command {
                HostPrecheckRunnerCommands::Install { target, json } => {
                    precheck_runner::install(&target, json).await
                }
                HostPrecheckRunnerCommands::Status { target, json } => {
                    precheck_runner::status(&target, json).await
                }
                HostPrecheckRunnerCommands::Remove { target, json } => {
                    precheck_runner::remove(&target, json).await
                }
                HostPrecheckRunnerCommands::RepositoryAdd {
                    repository,
                    runner_group,
                    json,
                } => {
                    precheck_runner::repository_add(&repository, runner_group.as_deref(), json)
                        .await
                }
                HostPrecheckRunnerCommands::ModelReviewAdd {
                    target,
                    repository,
                    json,
                } => precheck_runner::model_review_add(&target, &repository, json).await,
            },
            HostCommands::PublisherRunner(command) => match command {
                HostPublisherRunnerCommands::Install {
                    target,
                    repositories,
                    json,
                } => precheck_runner::install_publisher(&target, &repositories, json).await,
                HostPublisherRunnerCommands::RepositoryAdd { repository, json } => {
                    precheck_runner::publisher_repository_add(&repository, json).await
                }
                HostPublisherRunnerCommands::Bootstrap { repository, json } => {
                    precheck_runner::bootstrap_publisher_repository(&repository, json).await
                }
                HostPublisherRunnerCommands::DeveloperId {
                    target,
                    account_item,
                    repositories,
                    json,
                } => {
                    precheck_runner::bootstrap_developer_id(
                        &target,
                        &account_item,
                        &repositories,
                        json,
                    )
                    .await
                }
                HostPublisherRunnerCommands::Status { target, json } => {
                    precheck_runner::status_publisher(&target, json).await
                }
                HostPublisherRunnerCommands::Remove { target, json } => {
                    precheck_runner::remove_publisher(&target, json).await
                }
            },
            HostCommands::RemoveFile { target, path, json } => {
                host::remove_file(&target, &path, json).await
            }
            HostCommands::Cron {
                target,
                prune,
                restore,
                apply,
                json,
            } => host::cron(&target, prune.as_deref(), restore.as_deref(), apply, json).await,
            HostCommands::SyncAcquisitionScopes { target, source } => {
                host::sync_acquisition_scopes(&target, &source).await
            }
            HostCommands::RetagVaultItem {
                target,
                item,
                tags,
                json,
            } => host::retag_vault_item(&target, &item, tags.as_deref(), json).await,
            HostCommands::SyncVault { target, json } => host::sync_vault(&target, json).await,
            HostCommands::VaultItemPut {
                target,
                item,
                item_type,
                json,
            } => host::vault_item_put(&target, &item, &item_type, json).await,
            HostCommands::VaultTokenMint {
                target,
                consumer,
                capabilities,
                audience,
                ttl_seconds,
                replace_capabilities,
                raw_token,
                json,
            } => {
                host::vault_token_mint(
                    &target,
                    &consumer,
                    &capabilities,
                    &audience,
                    ttl_seconds,
                    replace_capabilities,
                    raw_token,
                    json,
                )
                .await
            }
            HostCommands::ReconcileObjectVerifier { target, json } => {
                host::reconcile_object_verifier(&target, json).await
            }
            HostCommands::ReconcileReleaseVerifier {
                target,
                product,
                json,
            } => host::reconcile_release_verifier(&target, &product, json).await,
            HostCommands::ReconcileServiceVerifier { target, json } => {
                host::reconcile_service_verifier(&target, json).await
            }
            HostCommands::RecoverSkarbiecAudit { target, json } => {
                host::recover_skarbiec_audit(&target, json).await
            }
            HostCommands::RecoverSkarbiecCrypto { target, json } => {
                host::recover_skarbiec_crypto(&target, json).await
            }
            HostCommands::RecoverSkarbiecAcquisitionState { target, json } => {
                host::recover_skarbiec_acquisition_state(&target, json).await
            }
            HostCommands::UnitLog {
                target,
                unit,
                lines,
                json,
            } => host::unit_log(&target, &unit, lines, json).await,
            HostCommands::VerifyReleasePlatform {
                target,
                repo,
                revision,
                json,
            } => host::verify_release_platform(&target, &repo, &revision, json).await,
            HostCommands::BackupAudit {
                target,
                reclaim_twins,
                apply,
                json,
            } => host::backup_audit(&target, reclaim_twins, apply, json).await,
            HostCommands::ForwardLocal {
                target,
                name,
                remote_port,
                local_port,
                json,
            } => host::forward_local(&target, &name, remote_port, local_port, json).await,
            HostCommands::ForwardClose { target, name, json } => {
                host::forward_close(&target, &name, json).await
            }
            HostCommands::ForwardRemote {
                target,
                name,
                remote_port,
                local_port,
                json,
            } => host::forward_remote(&target, &name, remote_port, local_port, json).await,
            HostCommands::Exec {
                target,
                json,
                command,
            } => host::exec(&target, command, json).await,
            HostCommands::JedenConnect {
                workspace,
                target,
                resume,
            } => coding::connect_jeden(&workspace, target.as_deref(), resume.as_deref()).await,
            HostCommands::DeclareVersion {
                target,
                binary,
                version,
                json,
            } => host::declare_version(&target, &binary, &version, json).await,
            HostCommands::PromoteVersion {
                binary,
                version,
                json,
            } => host::promote_version(&binary, &version, json).await,
            HostCommands::Reconcile {
                target,
                apply,
                json,
            } => host::reconcile(target, apply, json).await,
            HostCommands::Vaults { target, json } => host::vaults(target, json).await,
            HostCommands::Inventory { target, json } => host::inventory(&target, json).await,
            HostCommands::Software { target, json } => host::software(target, json).await,
            HostCommands::Provenance { target, json } => host::provenance(&target, json).await,
            HostCommands::WelesActivity { target, json } => {
                host::weles_activity(&target, json).await
            }
            HostCommands::WelesRunDiagnostics {
                target,
                run_id,
                file,
                json,
            } => host::weles_run_diagnostics(&target, &run_id, file.as_deref(), json).await,
            HostCommands::WelesImageInspect { target, url, json } => {
                host::weles_image_inspect(&target, &url, json).await
            }
            HostCommands::WelesCapture {
                target,
                plan,
                batch,
                json,
            } => host::weles_capture(&target, &plan, batch.as_deref(), json).await,
            HostCommands::WelesCaptureStatus {
                target,
                batch,
                json,
            } => host::weles_capture_status(&target, &batch, json).await,
            HostCommands::CapabilityRoute {
                target,
                resource,
                item,
                field,
                reason,
                verify,
                capability_file,
                routes_file,
                json,
            } => {
                host::capability_route(host::CapabilityRouteRequest {
                    target: &target,
                    resource: resource.as_deref(),
                    item: item.as_deref(),
                    field: field.as_deref(),
                    reason: reason.as_deref(),
                    verify,
                    capability_file: capability_file.as_deref(),
                    routes_file: routes_file.as_deref(),
                    json,
                })
                .await
            }
            HostCommands::ActivateStagedRelease {
                target,
                product,
                env_file,
                port,
                json,
            } => host::activate_staged_release(&target, &product, &env_file, port, json).await,
            HostCommands::WelesBrowserTask {
                target,
                url,
                objective,
                session_label,
                action,
                allowlist_file,
                login_item,
                account_id,
                fresh_profile,
                allow_login,
                sign_in_origin,
                sign_in_item,
                defer_fills,
                prefill_all,
                flow_name,
                windowed,
                json,
            } => {
                host::weles_browser_task(host::BrowserTaskRequest {
                    target: &target,
                    url: &url,
                    objective: &objective,
                    session_label: &session_label,
                    action: &action,
                    allowlist_file: &allowlist_file,
                    login_item: login_item.as_deref(),
                    account_id: account_id.as_deref(),
                    fresh_profile,
                    allow_login,
                    sign_in_origin: sign_in_origin.as_deref(),
                    sign_in_item: sign_in_item.as_deref(),
                    defer_fills,
                    prefill_all,
                    flow_name: flow_name.as_deref(),
                    windowed,
                    json,
                })
                .await
            }
            HostCommands::WelesBrowserRuntime {
                target,
                components,
                repair,
                json,
            } => host::weles_browser_runtime(&target, &components, repair, json).await,
            HostCommands::ConfigShow { target } => host::config_show(&target).await,
            HostCommands::ConfigSet {
                target,
                key,
                value,
                reload_service,
            } => host::config_set(&target, &key, &value, reload_service.as_deref()).await,
            HostCommands::Release {
                target,
                binary,
                version,
                dry_run,
                reinstall,
                json,
            } => host::release(&target, &binary, &version, dry_run, reinstall, json).await,
        },
        Commands::Bootstrap {
            target,
            dry_run,
            local,
        } => bootstrap::run(target, dry_run, local).await,
        Commands::Recovery(sub) => recovery::dispatch(sub).await,
        Commands::Storage(sub) => storage::dispatch(sub).await,
        Commands::Instances(sub) => instances::dispatch(sub).await,
        Commands::Secrets(sub) => secrets::dispatch(sub).await,
        Commands::Queue(sub) => queue::dispatch(sub).await,
        Commands::Alerts(sub) => alerts::dispatch(sub).await,
        Commands::Service(sub) => service::dispatch(sub).await,
        Commands::Egress(sub) => egress::dispatch(sub).await,
        Commands::Product(sub) => product::dispatch(sub).await,
        Commands::Placement(sub) => placement::dispatch(sub).await,
        Commands::Resolver(sub) => resolver::dispatch(sub).await,
        Commands::Database(sub) => database::dispatch(sub).await,
        Commands::Inference(sub) => inference::dispatch(sub).await,
        Commands::Stream(sub) => stream::dispatch(sub).await,
        Commands::Doctor(args) => doctor::dispatch(args).await,
    }
}

/// Identity bindings: which host holds what, and whether it still does.
#[derive(Subcommand)]
enum IdentityCommands {
    /// Every declared identity binding across the fleet.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Check a binding against the hosts themselves, not the declaration.
    Verify {
        #[arg(long)]
        kind: String,
        #[arg(long)]
        identity: String,
        #[arg(long)]
        json: bool,
    },
}
