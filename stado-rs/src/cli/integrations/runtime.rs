//! The host service owns its long-running components, not child daemons.

use std::num::{NonZeroU16, NonZeroU64};
use std::time::Duration;

use clap::Args;

use crate::cli::entry::spec::root::work::AgentOptions;
use crate::cli::hosts::{agent, coordinator};
use crate::cli::CmdError;
use crate::deploy::host_access::native::ReverseForward;

mod arguments;
mod api;
mod identity;
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
    #[arg(long, requires = "api")]
    pub api_storage: Option<crate::queue::ServerStorage>,
    /// Enable release reconciliation at this declared cadence.
    #[arg(long)]
    pub release_interval_seconds: Option<NonZeroU64>,
    /// Enable host-health publication at this declared cadence.
    #[arg(long)]
    pub health_interval_seconds: Option<NonZeroU64>,
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
    /// Collect workstation diagnostics inside this host process.
    #[arg(long)]
    pub watchdog: bool,
    /// Diagnostics destination; the standalone watchdog's default applies when omitted.
    #[arg(long, requires = "watchdog")]
    pub watchdog_bucket: Option<String>,
    /// Preserve the standalone watchdog's declared collection cadence.
    #[arg(
        long,
        requires = "watchdog",
        default_value_t = crate::watchdog::DEFAULT_INTERVAL_S,
        value_parser = clap::value_parser!(i64).range(crate::watchdog::MIN_INTERVAL_S..)
    )]
    pub watchdog_interval_seconds: i64,
}

pub(crate) async fn run(mut args: ServeArgs) -> Result<(), CmdError> {
    let serve_api = args.api;
    if !serve_api && (args.bind.is_some() || args.port.is_some() || args.api_storage.is_some()) {
        return Err(CmdError::usage(
            "serve --bind, --port and --api-storage require --api",
        ));
    }
    identity::validate(&args)?;
    let mutates_worker_environment = args.run_worker
        && (args.worker.auto || args.worker.target.is_none());
    let mut supervisor = supervisor::Supervisor::new();
    // Start an API before resolving host identity unless a worker first needs
    // to apply its environment. An API-only host needs no registry bootstrap.
    if serve_api && !mutates_worker_environment {
        let api = api::PreparedApi::prepare(args.bind.take(), args.port, args.api_storage.take()).await?;
        supervisor.spawn("api", move || api.run())?;
    }
    let target = supervisor.during_startup(identity::resolve(&mut args)).await?;

    let bundled_coordinator = supervisor.during_startup(async {
        match (args.control_plane, args.control_plane_interval_seconds) {
            (Some(mode), Some(interval)) => {
                let store = crate::queue::JobStorage::new().await
                    .map_err(|error| CmdError::click(error.to_string()))?;
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
    if let Some(interval) = args.release_interval_seconds {
        let release_target = identity::required_name(&target)?;
        supervisor.spawn("release", move || async move {
            crate::release_agent::agent(&release_target, None, false, interval.get()).await
        })?;
    }
    if let Some(interval) = args.health_interval_seconds {
        supervisor.spawn("host-health", move || {
            health_beacons(Duration::from_secs(interval.get()))
        })?;
    }
    if let Some((forward, interval)) = reverse_forward {
        supervisor.spawn("reverse-forward", move || forward.run(interval))?;
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
                .await.map_err(|error| CmdError::click(error.to_string()))
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
            )
        })?;
    }
    eprintln!(
        "stado serve: target={} pid={} components={}",
        target.as_ref().map(|target| target.name.as_str()).unwrap_or("not-required"),
        std::process::id(),
        supervisor.components().join(",")
    );
    supervisor.wait().await
}

async fn health_beacons(period: Duration) -> Result<(), CmdError> {
    let mut schedule = tokio::time::interval(period);
    schedule.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        schedule.tick().await;
        // Every collection runs to completion. A delayed pass skips obsolete
        // scheduled ticks instead of cancelling work or bursting repeated probes.
        if let Err(error) = crate::cli::host::collect_beacon(true).await {
            eprintln!("[stado serve host-health] collect-and-publish failed: {error}");
        }
    }
}
