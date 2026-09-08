//! The platform itself: its catalogs, its configuration, its hosts and the
//! services they run. The fourth and last block of `stado --help`.

use clap::Subcommand;

use crate::cli::*;

/// The fourth block of `stado` verbs. Flattened into
/// `super::super::Commands`, so splitting the declaration across files
/// changes no command line.
#[derive(Subcommand)]
pub(crate) enum PlatformCommands {
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

    /// Inspect or change stado configuration: show | validate | init | migrate | set | unset.
    Config {
        #[arg(default_value = "show")]
        sub: String,
        /// `set` and `unset`: dotted key, e.g. `alerts.channels`.
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
    /// Host a web product on the fleet: build it, run it, publish its hostname.
    #[command(subcommand)]
    Web(web::WebCommands),
    /// Own the records of a DNS zone Stado manages at its registrar.
    #[command(subcommand)]
    Dns(dns::DnsCommands),
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
    /// Place declared work on an eligible fleet host and return its stream or receipt.
    #[command(subcommand)]
    Workload(workload::WorkloadCommands),
    /// Inspect and apply the ordered repair steps services declare.
    Repair(repair::RepairArgs),
    /// Operate declared GitHub runner profiles across registry hosts.
    #[command(subcommand)]
    Runner(runner::RunnerCommands),
    /// Read and reclaim a host's declared space, including guarded file operations.
    #[command(subcommand)]
    Space(space::SpaceCommands),
    /// Inspect and operate service-directory routing without naming a product.
    #[command(subcommand)]
    Route(route::RouteCommands),
    /// Lease a disposable target on a registered host, and destroy it when the
    /// run ends.
    #[command(subcommand)]
    Scratch(scratch::ScratchCommands),
}
