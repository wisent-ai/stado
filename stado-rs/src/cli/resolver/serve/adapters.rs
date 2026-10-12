//! The resolver's listening adapters: each bound on a loopback address, and
//! the set made to match a changed directory without ending the process.

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::task::JoinSet;

use crate::cli::resolver::report::published::PublishedAdapter;
use crate::cli::resolver::serve::proxy::serve_adapter;
use crate::cli::resolver::serve::state::ResolverState;
use crate::cli::CmdError;
use crate::service_resolution::{self, ResolverAdapter};

/// The adapter among `adapters` bound at the address this process reads its
/// registry through (`storage.stado.url`), with the reason it cannot serve
/// it, when `refuses` says the process reads its registry that way.
///
/// Such a resolver cannot start that route in either order: with the object
/// API up the bind fails with "address already in use", with it down the
/// read fails with "error sending request". It used to stop the whole
/// resolver over it, and every other route on the host (Brama, Skarbiec)
/// went down with that one; now only this route is left unserved, named.
pub(super) fn self_served_store(
    adapters: &[ResolverAdapter],
    refuses: bool,
) -> Option<(ResolverAdapter, String)> {
    if !refuses {
        return None;
    }
    let store_url = crate::config::wc_stado_storage_url();
    let parsed = url::Url::parse(store_url.trim()).ok()?;
    let store_authority = format!("{}:{}", parsed.host_str()?, parsed.port()?);
    let adapter = adapters
        .iter()
        .find(|adapter| adapter.bind.trim() == store_authority)?;
    let reason = format!(
        "this resolver is declared to serve {} for service {:?}, and the registry it must read \
         first is configured at storage.stado.url = {}. One of the two has to move: either \
         place the object API somewhere this resolver does not serve, or drop that adapter from \
         the target's service_resolver policy; every other route is served.",
        adapter.bind, adapter.service, store_url
    );
    Some((adapter.clone(), reason))
}

/// Bind every adapter in `adapters` but the one `refused` names. An adapter
/// whose bind fails, or the refused one, is answered unserved with its own
/// reason, so one held port or one contradictory declaration leaves every
/// other route on the host served.
pub(super) async fn bind_adapters(
    adapters: &[ResolverAdapter],
    refused: Option<(ResolverAdapter, String)>,
) -> (Vec<(ResolverAdapter, TcpListener)>, Vec<PublishedAdapter>) {
    let mut bound = Vec::with_capacity(adapters.len());
    let mut unserved = Vec::new();
    for adapter in adapters {
        let failure = match refused.as_ref().filter(|(named, _)| named == adapter) {
            Some((_, reason)) => Err(reason.clone()),
            None => bind_loopback(&adapter.bind)
                .await
                .map_err(|error| error.to_string()),
        };
        match failure {
            Ok(listener) => bound.push((adapter.clone(), listener)),
            Err(reason) => {
                eprintln!(
                    "stado resolver does not serve adapter service={} consumer={} bind={}: {reason}",
                    adapter.service, adapter.consumer, adapter.bind
                );
                unserved.push(PublishedAdapter {
                    service: adapter.service.clone(),
                    consumer: adapter.consumer.clone(),
                    bind: adapter.bind.clone(),
                    unserved: Some(reason),
                });
            }
        }
    }
    (bound, unserved)
}

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
/// A bind that fails leaves that adapter unserved with its reason, as at
/// startup, and the next directory change tries it again.
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
    let added: Vec<ResolverAdapter> = next
        .adapters
        .iter()
        .filter(|adapter| !kept.contains(adapter))
        .cloned()
        .collect();
    let refused = self_served_store(&added, state.refuses_store_adapter);
    let (bound, unserved) = bind_adapters(&added, refused).await;
    let mut serving: Vec<ResolverAdapter> = kept.into_iter().cloned().collect();
    for (adapter, listener) in bound {
        let adapter_state = Arc::clone(state);
        let owned = adapter.clone();
        let handle =
            tasks.spawn(async move { serve_adapter(listener, owned, adapter_state).await });
        listening.insert(adapter.bind.clone(), handle);
        eprintln!(
            "stado resolver started adapter service={} consumer={} bind={}",
            adapter.service, adapter.consumer, adapter.bind
        );
        serving.push(adapter);
    }
    state.hold(serving, unserved);
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
