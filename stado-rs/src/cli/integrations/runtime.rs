//! The host service owns its long-running components, not child daemons.

use std::time::Duration;

use crate::cli::hosts::{agent, coordinator};
use crate::cli::CmdError;
use crate::deploy::host_access::native::ReverseForward;

mod api;
mod arguments;
pub(crate) use arguments::ServeArgs;
mod delivery;
mod identity;
mod own_route;
mod precheck_runner;
pub(crate) mod roles;
pub(crate) mod stops;
mod supervisor;

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
    if args.inherited_listener && (args.bind.is_some() || args.port.is_some()) {
        return Err(CmdError::usage(
            "serve --inherited-listener serves the socket on standard input; --bind and --port \
             name a socket to bind and cannot be combined with it",
        ));
    }
    if let Some(root) = args.api_local_store.take() {
        args.api_storage = Some(crate::queue::ServerStorage::local(root));
    }
    identity::validate(&args)?;
    // One process per role on a host: the lock is held for this process's
    // life (`_role`), and a second `serve` for the same target is refused
    // here, before any listener, resolver or worker of it starts.
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| {
            CmdError::declaration("HOME is not set, so this host's role locks cannot be found")
        })?;
    let program = std::env::current_exe().map_err(|error| {
        CmdError::unreachable(format!(
            "this program cannot name its own executable: {error}"
        ))
    })?;
    let role = crate::deploy::role_lock::serve_role(args.worker.target.as_deref());
    let _role = crate::deploy::role_lock::take(&home, &role, &program)?;
    let mutates_worker_environment =
        args.run_worker && (args.worker.auto || args.worker.target.is_none());
    let mut supervisor = supervisor::Supervisor::new();
    // Delivery first: product sync installs the release that replaces this
    // one, so no other role's start or stop may end it.
    if let Some(interval) = args.product_sync_interval_seconds {
        let surfaces = std::mem::take(&mut args.product_sync_surface);
        supervisor.spawn_delivery("product-sync", move || {
            delivery::product_sync(Duration::from_secs(interval.get()), surfaces)
        })?;
    }
    let started = start_roles(&mut supervisor, args, serve_api, mutates_worker_environment).await;
    match started {
        Ok(()) => supervisor.until_ended().await,
        Err(failure) => supervisor.deliver_after(failure).await,
    }
}

/// Every role after delivery, in their order. An error ends the start, and
/// the caller keeps delivering ([`supervisor::Supervisor::deliver_after`]).
async fn start_roles(
    supervisor: &mut supervisor::Supervisor,
    mut args: ServeArgs,
    serve_api: bool,
    mutates_worker_environment: bool,
) -> Result<(), CmdError> {
    // Start an API before resolving host identity unless a worker first needs
    // to apply its environment. An API-only host needs no registry bootstrap.
    if serve_api && !mutates_worker_environment {
        let api = api::PreparedApi::prepare(api::ApiShape {
            bind: args.bind.take(),
            port: args.port,
            storage: args.api_storage.take(),
            enrollment_only: args.enrollment_only,
            inherited_listener: args.inherited_listener,
        })
        .await?;
        supervisor.spawn("api", move || api.run())?;
    }
    let target = supervisor
        .during_startup(identity::resolve(&mut args))
        .await?;
    if args.resolver {
        let resolver_target = identity::required_name(&target)?;
        supervisor.spawn("resolver", move || async move {
            crate::cli::resolver::serve(&resolver_target).await
        })?;
    }

    let bundled_coordinator = supervisor.during_startup(async {
        match (args.control_plane, args.control_plane_interval_seconds) {
            (Some(mode), Some(interval)) => {
                let store = crate::queue::JobStorage::new().await.map_err(CmdError::from)?;
                let coordinator = crate::remote::control_plane::ResidentCoordinator::prepare(mode, store, interval).await
                    .map_err(CmdError::from)?;
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
            ReverseForward::new(destination, remote, local).map_err(|error| {
                CmdError::click(format!("{error:#}")).stating(crate::primitives::failure::FailureCode::Config)
            })?,
            interval,
        )),
        (None, None, None, None) => None,
        _ => return Err(CmdError::usage("serve reverse forwarding requires a destination, both loopback ports and its reconciliation interval")),
    };
    let api = if serve_api && mutates_worker_environment {
        Some(
            api::PreparedApi::prepare(api::ApiShape {
                bind: args.bind.take(),
                port: args.port,
                storage: args.api_storage.take(),
                enrollment_only: args.enrollment_only,
                inherited_listener: args.inherited_listener,
            })
            .await?,
        )
    } else {
        None
    };

    // The stable-port proxies belong to release reconciliation, so only a
    // process that reconciles releases owns their control socket: it is one
    // per user on a host, and a second serve process on the same host (an
    // enrollment listener, a standalone device) must not contend for it.
    if args.release_interval_seconds.is_some() {
        let proxy_control =
            crate::release_agent::rollout::serving::control::prepare().map_err(CmdError::from)?;
        supervisor.spawn("release-proxy", move || async move {
            crate::release_agent::rollout::serving::control::serve(proxy_control)
                .await
                .map_err(CmdError::from)
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
        // own local API published where no fleet reader looked. The store is
        // opened by the role, not at startup: opening it reads the store, and
        // a fleet store behind another host's object API answering `503
        // object authorization unavailable` ended this whole process — its
        // resolver, release proxy and worker with it — on every restart.
        supervisor.spawn("host-health", move || {
            crate::cli::host::collect_beacons_every(Duration::from_secs(interval.get()))
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
            let reason = crate::watchdog::run(&diagnostics).await;
            Err::<(), _>(CmdError::unreachable(format!(
                "workstation diagnostics stopped: {reason}"
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
            Err::<(), _>(CmdError::unreachable("bundled coordinator returned"))
        })?;
    }
    if let Some(api) = api {
        supervisor.spawn("api", move || api.run())?;
    }
    if args.disk_cleanup {
        // `--disk-cleanup` requires `--health-interval-seconds`.
        if let Some(every) = args.health_interval_seconds {
            supervisor.spawn("disk-cleanup", move || {
                crate::cli::hosts::disk_cleanup::watch(every)
            })?;
        }
    }
    if let Some(interval) = args.failure_fixer_interval_seconds {
        supervisor.spawn("failure-fixer", move || async move {
            crate::failure_fixer::run_resident(interval, args.failure_fixer_command_pattern)
                .await
                .map_err(CmdError::from)
        })?;
    }
    if args.run_worker {
        let idle_shutdown = args.worker.idle_shutdown;
        let worker = move || {
            agent::run(
                args.worker.gpu_type,
                idle_shutdown,
                args.worker.kind,
                args.worker.vast_auto_list,
                args.worker.vast_price_gpu,
                args.worker.vast_max_duration_s,
                args.worker.vast_idle_window_s,
                args.worker.poll_seconds,
            )
        };
        // An ephemeral machine's worker ends when no eligible work remains,
        // and that ending is this process's success.
        if idle_shutdown {
            supervisor.spawn_finite("worker", worker)?;
        } else {
            supervisor.spawn("worker", worker)?;
        }
    }
    // A serve line without a worker target or resolver needs no registry
    // target, and says so.
    let named = match target.as_ref() {
        Some(target) => target.name.as_str(),
        None => "not-required",
    };
    eprintln!(
        "stado serve: target={named} pid={} components={}",
        std::process::id(),
        supervisor.components().join(",")
    );
    Ok(())
}
