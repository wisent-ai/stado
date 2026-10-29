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
