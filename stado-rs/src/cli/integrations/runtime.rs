//! The host service owns its long-running components, not child daemons.

use std::time::Duration;

use crate::cli::hosts::{agent, coordinator};
use crate::cli::CmdError;
use crate::deploy::host_access::native::ReverseForward;

mod api;
mod arguments;
pub(crate) use arguments::ServeArgs;
mod delivery;
mod precheck_runner;
mod startup;
use startup::{identity, own_route};
mod recorded;
pub(crate) use recorded::serve_line;
pub(crate) mod roles;
mod start;
pub(crate) mod stops;
mod supervisor;
use start::start_roles;
pub(crate) use supervisor::Served;

pub(crate) async fn run(args: ServeArgs) -> Result<Served, CmdError> {
    // The tailnet map is read beside everything serve does, never on its
    // path: a Tailscale client that does not answer holds no step and no role.
    crate::remote::tailnet::read_beside();
    // The client that read started is serve's own: when serve is told to end
    // (SIGTERM from its init system or a successor's takeover, SIGINT,
    // SIGHUP), it ends that client first and then ends as the signal says.
    // A serve that ends on its own path ends its client the same way.
    let ending = until_signalled()?;
    let served = tokio::select! {
        served = serve(args) => served,
        signal = ending => {
            end_tailnet_reader().await;
            return Err(ended_by(signal));
        }
    };
    end_tailnet_reader().await;
    served
}

/// End the tailnet reader's client, saying on stderr when it cannot be.
async fn end_tailnet_reader() {
    if let Err(error) = crate::remote::tailnet::end().await {
        eprintln!("stado serve: {error}");
    }
}

/// The future of the first ending signal this process receives, listening
/// from the moment this returns. A signal that cannot be listened for is the
/// error naming it, before serve starts anything.
fn until_signalled() -> Result<impl std::future::Future<Output = nix::sys::signal::Signal>, CmdError>
{
    use nix::sys::signal::Signal;
    use tokio::signal::unix::{signal, SignalKind};
    let listen = |kind: SignalKind| {
        signal(kind).map_err(|error| {
            CmdError::click(format!(
                "stado serve could not listen for the signals that end it: {error}"
            ))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })
    };
    let mut terminate = listen(SignalKind::terminate())?;
    let mut interrupt = listen(SignalKind::interrupt())?;
    let mut hangup = listen(SignalKind::hangup())?;
    Ok(async move {
        tokio::select! {
            _ = terminate.recv() => Signal::SIGTERM,
            _ = interrupt.recv() => Signal::SIGINT,
            _ = hangup.recv() => Signal::SIGHUP,
        }
    })
}

/// End this process by `signal` with its default action, as it would have
/// ended had serve not listened for it: its init system reads the same end.
/// Answers only when the signal could not be delivered, naming why.
fn ended_by(signal: nix::sys::signal::Signal) -> CmdError {
    use nix::sys::signal::{raise, sigaction, SaFlags, SigAction, SigHandler, SigSet};
    let default = SigAction::new(SigHandler::SigDfl, SaFlags::empty(), SigSet::empty());
    // SAFETY: the default disposition installs no handler of this program's.
    let restored = unsafe { sigaction(signal, &default) };
    let raised = restored.and_then(|_| raise(signal));
    CmdError::click(match raised {
        Ok(()) => format!("stado serve received {signal} and did not end by it"),
        Err(errno) => format!("stado serve received {signal} and could not end by it: {errno}"),
    })
}

async fn serve(mut args: ServeArgs) -> Result<Served, CmdError> {
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
    let home = crate::config_file::home().map_err(|cause| {
        CmdError::declaration(format!("this host's role locks cannot be found: {cause}"))
    })?;
    let program = std::env::current_exe().map_err(|error| {
        CmdError::unreachable(format!(
            "this program cannot name its own executable: {error}"
        ))
    })?;
    let role = crate::deploy::role_lock::serve_role(args.worker.target.as_deref());
    // A held role on an API start may be this unit's own predecessor, which
    // only the API's takeover retires; it runs before the lock, or the lock
    // refuses the start the takeover exists for.
    if serve_api
        && !args.inherited_listener
        && crate::deploy::role_lock::holder(&home, &role)?.is_some()
    {
        api::take_over_held_role(args.api_storage.as_ref()).await?;
    }
    let _role = crate::deploy::role_lock::take(&home, &role, &program)?;
    let mutates_worker_environment =
        args.run_worker && (args.worker.auto || args.worker.target.is_none());
    let mut supervisor = supervisor::Supervisor::new();
    // Delivery first: product sync installs the release that replaces this
    // one, so no other role's start or stop may end it.
    if let Some(interval) = args.product_sync_interval_seconds {
        let surfaces = std::mem::take(&mut args.product_sync_surface);
        let watch = args
            .health_interval_seconds
            .map(|seconds| Duration::from_secs(seconds.get()));
        supervisor.spawn_delivery("product-sync", move || {
            delivery::product_sync(Duration::from_secs(interval.get()), watch, surfaces)
        })?;
    }
    let started = start_roles(&mut supervisor, args, serve_api, mutates_worker_environment).await;
    match started {
        Ok(()) => supervisor.until_ended().await,
        Err(failure) => Err(supervisor.deliver_after(failure).await),
    }
}
