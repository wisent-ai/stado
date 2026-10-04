//! The host service owns its long-running components, not child daemons.

use std::num::{NonZeroU16, NonZeroU64};
use std::time::Duration;

use clap::Args;

use crate::cli::entry::spec::root::work::AgentOptions;
use crate::cli::hosts::{agent, coordinator};
use crate::cli::CmdError;
use crate::deploy::host_access::native::ReverseForward;

mod api;
mod arguments;
mod identity;
mod own_route;
mod precheck_runner;
pub(crate) mod roles;
mod supervisor;

#[derive(Args)]
pub(crate) struct ServeArgs {
    #[command(flatten)]
    pub worker: AgentOptions,
    /// Run a persistent local worker; other host roles do not imply one.
    #[arg(long = "worker")]
    pub run_worker: bool,
    /// Run the declared disk and memory cleanup watch inside this host process.
    #[arg(long)]
    pub disk_cleanup: bool,
    /// Dispatch unhandled failed jobs inside this process at the declared cadence.
    #[arg(long)]
    pub failure_fixer_interval_seconds: Option<NonZeroU64>,
    /// Preserve the failure fixer's command substring filter.
    #[arg(long, requires = "failure_fixer_interval_seconds")]
    pub failure_fixer_command_pattern: Option<String>,
    /// Run this host's declared service resolver inside the host process.
    #[arg(long)]
    pub resolver: bool,
    /// Exact coordinator entry to run here; add --api to expose its API listener.
    #[arg(long, conflicts_with = "control_plane")]
    pub coordinator: Option<String>,
    /// Preserve a bundled local or cloud scheduling loop inside this process.
    #[arg(long, value_enum, requires = "control_plane_interval_seconds")]
    pub control_plane: Option<crate::remote::control_plane::CoordinatorMode>,
    /// Scheduling cadence from the existing bundled control-plane declaration.
    #[arg(long, requires = "control_plane")]
    pub control_plane_interval_seconds: Option<i64>,
    /// Serve the configured API without claiming a coordinator role.
    #[arg(long)]
    pub api: bool,
    /// API bind address, using the existing dashboard configuration when omitted.
    #[arg(long)]
    pub bind: Option<String>,
    /// API port, using the existing dashboard configuration when omitted.
    #[arg(long)]
    pub port: Option<u16>,
    /// JSON primary/backup endpoints for the API, independent of worker storage.
    #[arg(long, requires = "api", conflicts_with = "api_local_store")]
    pub api_storage: Option<crate::queue::ServerStorage>,
    /// The local root the API serves, independent of the storage the other
    /// roles use: `--api-storage` for a local primary, spelled as a path so a
    /// unit file can carry it (a unit argument cannot hold the JSON's quotes).
    #[arg(long, requires = "api", value_name = "PATH")]
    pub api_local_store: Option<std::path::PathBuf>,
    /// Enable release reconciliation at this declared cadence.
    #[arg(long)]
    pub release_interval_seconds: Option<NonZeroU64>,
    /// Enable host-health publication at this declared cadence.
    #[arg(long)]
    pub health_interval_seconds: Option<NonZeroU64>,
    /// Reconcile installed product surfaces against canonical origin/main at
    /// this declared cadence, inside this process.
    #[arg(long, requires = "product_sync_surface")]
    pub product_sync_interval_seconds: Option<NonZeroU64>,
    /// A surface `stado product sync --surface` reconciles on that cadence;
    /// repeat for each declared surface, in the order they run.
    #[arg(long, requires = "product_sync_interval_seconds")]
    pub product_sync_surface: Vec<String>,
    /// Existing reverse-forward SSH destination (user@host); both ends stay loopback.
    #[arg(long, requires_all = ["forward_remote_port", "forward_local_port", "forward_interval_seconds"])]
    pub forward_destination: Option<String>,
    /// Remote loopback port of the existing reverse listener.
    #[arg(long, requires = "forward_destination")]
    pub forward_remote_port: Option<NonZeroU16>,
    /// Local loopback port reached by that listener.
    #[arg(long, requires = "forward_destination")]
    pub forward_local_port: Option<NonZeroU16>,
    /// Reconcile the reverse listener at its own declared cadence.
    #[arg(long, requires = "forward_destination")]
    pub forward_interval_seconds: Option<NonZeroU64>,
    /// Run the web edge's reverse proxy, this program, inside this process.
    #[arg(long, requires = "edge_caddyfile")]
    pub edge_caddy: Option<std::path::PathBuf>,
    /// The Caddyfile `stado web edge` delivers to this host; watched and reloaded.
    #[arg(long, requires = "edge_caddy")]
    pub edge_caddyfile: Option<std::path::PathBuf>,
    /// Run the GitHub pre-check runner installed at this root inside this
    /// process: its launcher starts under passwordless sudo and drops to the
    /// runner's own account.
    #[arg(long, value_name = "ROOT")]
    pub precheck_runner: Option<std::path::PathBuf>,
    /// Collect workstation diagnostics inside this host process.
    #[arg(long)]
    pub watchdog: bool,
    /// Diagnostics destination; the standalone watchdog's default applies when omitted.
    #[arg(long, requires = "watchdog")]
    pub watchdog_bucket: Option<String>,
    /// Seconds between workstation diagnostics collections; required with
    /// --watchdog.
    #[arg(long, requires = "watchdog")]
    pub watchdog_interval_seconds: Option<i64>,
}

pub(crate) async fn run(mut args: ServeArgs) -> Result<(), CmdError> {
    let serve_api = args.api;
    if !serve_api
        && (args.bind.is_some()
            || args.port.is_some()
            || args.api_storage.is_some()
            || args.api_local_store.is_some())
    {
        return Err(CmdError::usage(
            "serve --bind, --port, --api-storage and --api-local-store require --api",
        ));
    }
    if let Some(root) = args.api_local_store.take() {
        args.api_storage = Some(crate::queue::ServerStorage::local(root));
    }
    identity::validate(&args)?;
    let mutates_worker_environment =
        args.run_worker && (args.worker.auto || args.worker.target.is_none());
    let mut supervisor = supervisor::Supervisor::new();
    // Start an API before resolving host identity unless a worker first needs
    // to apply its environment. An API-only host needs no registry bootstrap.
    if serve_api && !mutates_worker_environment {
        let api =
            api::PreparedApi::prepare(args.bind.take(), args.port, args.api_storage.take()).await?;
        supervisor.spawn("api", move || api.run())?;
    }
    let target = supervisor
        .during_startup(identity::resolve(&mut args))
        .await?;

    let bundled_coordinator = supervisor.during_startup(async {
        match (args.control_plane, args.control_plane_interval_seconds) {
            (Some(mode), Some(interval)) => {
                let store = crate::queue::JobStorage::new().await.map_err(CmdError::from)?;
                let coordinator = crate::remote::control_plane::ResidentCoordinator::prepare(mode, store, interval).await
                    .map_err(|error| CmdError::click(error.to_string()))?;
                Ok(Some(coordinator))
            }
            (None, None) => Ok(None),
            _ => Err(CmdError::usage("serve --control-plane and --control-plane-interval-seconds must be supplied together")),
        }
    }).await?;
    let reverse_forward = match (
        args.forward_destination,
        args.forward_remote_port,
        args.forward_local_port,
        args.forward_interval_seconds,
    ) {
        (Some(destination), Some(remote), Some(local), Some(interval)) => Some((
            ReverseForward::new(destination, remote, local)
                .map_err(|error| CmdError::click(error.to_string()))?,
            interval,
        )),
        (None, None, None, None) => None,
        _ => return Err(CmdError::usage("serve reverse forwarding requires a destination, both loopback ports and its reconciliation interval")),
    };
    let api = if serve_api && mutates_worker_environment {
        Some(api::PreparedApi::prepare(args.bind.take(), args.port, args.api_storage.take()).await?)
    } else {
        None
    };

    let proxy_control =
        crate::release_agent::rollout::serving::control::prepare().map_err(CmdError::click)?;
    supervisor.spawn("release-proxy", move || {
        crate::release_agent::rollout::serving::control::serve(proxy_control)
    })?;
    if args.resolver {
        let resolver_target = identity::required_name(&target)?;
        supervisor.spawn("resolver", move || async move {
            crate::cli::resolver::serve(&resolver_target).await
        })?;
    }
    // The roles below read the store; when it is behind this process's own
    // resolver they start once it serves.
    supervisor
        .during_startup(own_route::await_own_resolver(args.resolver))
        .await?;
    if let Some(interval) = args.release_interval_seconds {
        let release_target = identity::required_name(&target)?;
        supervisor.spawn("release", move || async move {
            crate::release_agent::agent(&release_target, None, false, interval.get()).await
        })?;
    }
    if let Some(interval) = args.health_interval_seconds {
        // The beacon goes into the fleet store this process's queue roles
        // use — the store every reader of `host_health/` reads — not into a
        // local store this process may serve as an API: a host serving its
        // own local API published where no fleet reader looked.
        let store = crate::queue::JobStorage::new().await.map_err(CmdError::from)?;
        supervisor.spawn("host-health", move || {
            health_beacons(Duration::from_secs(interval.get()), store)
        })?;
    }
    if let Some(interval) = args.product_sync_interval_seconds {
        let surfaces = args.product_sync_surface;
        supervisor.spawn("product-sync", move || {
            product_sync(Duration::from_secs(interval.get()), surfaces)
        })?;
    }
    if let Some((forward, interval)) = reverse_forward {
        supervisor.spawn("reverse-forward", move || forward.run(interval))?;
    }
    if let (Some(caddy), Some(caddyfile)) = (args.edge_caddy, args.edge_caddyfile) {
        supervisor.spawn("edge", move || crate::cli::web::edge_role(caddy, caddyfile))?;
    }
    if let Some(root) = args.precheck_runner {
        supervisor.spawn("precheck-runner", move || precheck_runner::run(root))?;
    }
    if args.watchdog {
        let diagnostics = crate::watchdog::ParsedArgs {
            bucket: args
                .watchdog_bucket
                .unwrap_or_else(crate::watchdog::configured_bucket),
            interval_s: args.watchdog_interval_seconds,
            once: false,
        };
        supervisor.spawn("watchdog", move || async move {
            let code = crate::watchdog::run(&diagnostics).await;
            Err::<(), _>(CmdError::click(format!(
                "workstation diagnostics returned exit status {code}"
            )))
        })?;
    }
    if let Some(name) = args.coordinator {
        // Only a present worker can own the drained image-replacement handshake.
        // A coordinator-only host retains its existing self-update path.
        let invocation = if args.run_worker {
            crate::coordinator::Invocation::Hosted
        } else {
            crate::coordinator::Invocation::Daemon
        };
        supervisor.spawn("coordinator", move || {
            coordinator::run(Some(name), invocation)
        })?;
    }
    if let Some(coordinator) = bundled_coordinator {
        supervisor.spawn("coordinator", move || async move {
            coordinator.run().await;
            Err::<(), _>(CmdError::click("bundled coordinator returned"))
        })?;
    }
    if let Some(api) = api {
        supervisor.spawn("api", move || api.run())?;
    }
    if args.disk_cleanup {
        supervisor.spawn("disk-cleanup", || {
            crate::cli::hosts::disk_cleanup::run(false, true, false, false)
        })?;
    }
    if let Some(interval) = args.failure_fixer_interval_seconds {
        supervisor.spawn("failure-fixer", move || async move {
            crate::failure_fixer::run_resident(interval, args.failure_fixer_command_pattern)
                .await
                .map_err(|error| CmdError::click(error.to_string()))
        })?;
    }
    if args.run_worker {
        supervisor.spawn("worker", move || {
            agent::run(
                args.worker.gpu_type,
                None,
                false,
                false,
                args.worker.kind,
                args.worker.vast_auto_list,
                args.worker.vast_price_gpu,
                args.worker.vast_max_duration_s,
                args.worker.vast_idle_window_s,
                args.worker.poll_seconds,
            )
        })?;
    }
    eprintln!(
        "stado serve: target={} pid={} components={}",
        target
            .as_ref()
            .map(|target| target.name.as_str())
            .unwrap_or("not-required"),
        std::process::id(),
        supervisor.components().join(",")
    );
    supervisor.wait().await
}

async fn health_beacons(period: Duration, store: crate::queue::JobStorage) -> Result<(), CmdError> {
    let mut schedule = tokio::time::interval(period);
    schedule.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        schedule.tick().await;
        // Every collection runs to completion. A delayed pass skips obsolete
        // scheduled ticks instead of cancelling work or bursting repeated probes.
        let destination = crate::cli::host::BeaconDestination::Store(&store);
        if let Err(error) = crate::cli::host::collect_beacon_to(destination).await {
            eprintln!("[stado serve host-health] collect-and-publish failed: {error}");
        }
    }
}

/// `stado product`, as the CLI defines it.
fn product_command() -> clap::Command {
    stado_product::cli::augment(clap::Command::new("product"))
}

async fn product_sync(period: Duration, surfaces: Vec<String>) -> Result<(), CmdError> {
    let mut schedule = tokio::time::interval(period);
    schedule.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        schedule.tick().await;
        for surface in &surfaces {
            let line = vec![
                "product".to_string(),
                "sync".to_string(),
                "--surface".to_string(),
                surface.clone(),
                "--fetch".to_string(),
            ];
            let outcome = tokio::task::spawn_blocking(move || {
                let matches = product_command()
                    .try_get_matches_from(line)
                    .map_err(|error| anyhow::anyhow!("{error}"))?;
                stado_product::cli::run(matches, crate::cli::setup::product::build())
            })
            .await;
            match outcome {
                Ok(Ok(0)) => {}
                Ok(Ok(status)) => eprintln!(
                    "[stado serve product-sync] {surface} sync exited with status {status}"
                ),
                Ok(Err(error)) => {
                    eprintln!("[stado serve product-sync] {surface} sync failed: {error:#}")
                }
                Err(error) => {
                    eprintln!("[stado serve product-sync] {surface} sync stopped: {error}")
                }
            }
        }
    }
}
