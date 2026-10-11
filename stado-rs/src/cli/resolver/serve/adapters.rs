//! The resolver's listening adapters: each bound on a loopback address, and
//! the set made to match a changed directory without ending the process.

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::task::JoinSet;

use crate::cli::resolver::serve::proxy::serve_adapter;
use crate::cli::resolver::serve::state::ResolverState;
use crate::cli::CmdError;
use crate::service_resolution;

/// Make the listening adapters match `next` without ending the process.
///
/// A changed directory used to end the resolver ("resolver configuration
/// changed; restarting to rebind listeners"), and because the resolver is a
/// role of `stado serve`, the whole process with it: every forward on the
/// host closed, mid-stream, on every registry change - a five-minute Weles
/// sign-in streamed through 127.0.0.1:17690 died with "unexpected EOF during
/// chunk size line" when a release elsewhere advanced the directory. Only the
/// adapters whose declaration changed are rebound now: one that is no longer
/// declared, or declared differently, has its accept loop stopped (open
/// connections are their own tasks and finish), and one that is new is bound.
/// A bind that fails ends the resolver with the holder named, as at startup.
pub(super) async fn reconcile_adapters(
    state: &Arc<ResolverState>,
    next: &service_resolution::ResolverConfig,
    listening: &mut std::collections::HashMap<String, tokio::task::AbortHandle>,
    tasks: &mut JoinSet<Result<(), String>>,
) -> Result<(), CmdError> {
    let current: Vec<service_resolution::ResolverAdapter> = state
        .adapters
        .read()
        .map(|held| held.clone())
        .unwrap_or_default();
    let kept: Vec<&service_resolution::ResolverAdapter> = current
        .iter()
        .filter(|adapter| next.adapters.contains(adapter))
        .collect();
    // The roles beside this one built their store client on the address this
    // process published; moving that adapter would leave them on a closed
    // port, so that one change still restarts the process.
    let store = crate::config::wc_stado_storage_url();
    if let Some(adapter) = current.iter().find(|adapter| {
        !kept.contains(adapter) && store.trim_end_matches('/') == format!("http://{}", adapter.bind)
    }) {
        // A declaration change this process cannot apply in place: config,
        // so the restart reads as the registry's doing, not a crash.
        return Err(CmdError::declaration(format!(
            "the {}/{} adapter at {} that this process reads its store through changed; \
             restarting so every role reads the new address",
            adapter.service, adapter.consumer, adapter.bind
        )));
    }
    for adapter in current.iter().filter(|adapter| !kept.contains(adapter)) {
        if let Some(handle) = listening.remove(&adapter.bind) {
            handle.abort();
        }
        eprintln!(
            "stado resolver stopped adapter service={} consumer={} bind={}",
            adapter.service, adapter.consumer, adapter.bind
        );
    }
    for adapter in next
        .adapters
        .iter()
        .filter(|adapter| !kept.contains(adapter))
    {
        let listener = bind_loopback(&adapter.bind).await?;
        let adapter_state = Arc::clone(state);
        let owned = adapter.clone();
        let handle =
            tasks.spawn(async move { serve_adapter(listener, owned, adapter_state).await });
        listening.insert(adapter.bind.clone(), handle);
        eprintln!(
            "stado resolver started adapter service={} consumer={} bind={}",
            adapter.service, adapter.consumer, adapter.bind
        );
    }
    if let Ok(mut held) = state.adapters.write() {
        *held = next.adapters.clone();
    }
    Ok(())
}

/// Bind the loopback address `value` names; any other address is refused,
/// and one already held is refused naming how to find its holder.
pub(super) async fn bind_loopback(value: &str) -> Result<TcpListener, CmdError> {
    let address: SocketAddr = value
        .parse()
        .map_err(|_| CmdError::usage(format!("invalid resolver bind {value:?}")))?;
    if !address.ip().is_loopback() {
        return Err(CmdError::refused(format!(
            "resolver bind {value:?} must be loopback"
        )));
    }
    TcpListener::bind(address).await.map_err(|error| {
        // A stable port held by something else is the failure this resolver
        // cannot recover from by waiting: a stale forward left behind by an
        // earlier instance accepts connections and answers none, so every
        // reader sees a live socket in front of nothing. Name the holder here
        // rather than retrying into it.
        CmdError::click(format!(
            "could not bind {value}: {error}. Something else already holds this \
             resolver port; find it with `lsof -nP -iTCP:{} -sTCP:LISTEN` and stop \
             it before starting the resolver.",
            address.port()
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })
}
