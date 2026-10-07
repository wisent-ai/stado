use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tokio::task::JoinSet;

use crate::service_resolution;
use crate::targets::RegistryStore;

use crate::cli::CmdError;

mod api;
mod proxy;
mod startup;
mod state;

use crate::cli::resolver::report::published::now_iso;
use crate::cli::resolver::report::published::publish;
use crate::cli::resolver::report::published::PublishedState;
use crate::cli::resolver::serve::api::serve_api;
use crate::cli::resolver::serve::proxy::serve_adapter;
use crate::cli::resolver::serve::startup::await_startup;
use crate::cli::resolver::serve::startup::Startup;
use crate::cli::resolver::serve::state::ResolverState;
use crate::cli::resolver::serve::state::Snapshot;

pub async fn serve(target: &str) -> Result<(), CmdError> {
    let local_store = match RegistryStore::open().await {
        Ok(store) => Some(Arc::new(store)),
        Err(error) => {
            eprintln!("stado resolver recovery: registry backend construction failed: {error}");
            None
        }
    };
    let Startup {
        source,
        document,
        store_version,
        directory_generation,
        recovered,
    } = await_startup(target, local_store.as_ref()).await?;
    let config = match service_resolution::resolver_config(&document, target) {
        Ok(config) => config,
        Err(detail) => {
            publish(&PublishedState::failed(target, &detail));
            return Err(
                CmdError::click(detail).stating(crate::primitives::failure::FailureCode::Config)
            );
        }
    };
    let state = Arc::new(ResolverState {
        local_store,
        source: RwLock::new(source),
        snapshot: RwLock::new(Snapshot {
            document,
            store_version,
            directory_generation,
            loaded_at: Instant::now(),
            loaded_at_iso: now_iso(),
        }),
        max_stale: Duration::from_secs(config.max_stale_seconds),
        local_target: target.to_string(),
        adapters: std::sync::RwLock::new(config.adapters.clone()),
        config: std::sync::RwLock::new(config.clone()),
        tunnels: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        // Startup has not written a copy yet; the first `refresh` sets this
        // either way.
        last_good_refusal: RwLock::new(None),
    });

    // A resolver that must serve the very address it reads the registry through
    // cannot start in either order, and the two failures look unrelated: with
    // the object API up the bind fails with "address already in use", with it
    // down the read fails with "error sending request". On this workstation that
    // alternation ran 641 restarts while the desktop app quietly fell back to a
    // local vault and showed no subscriptions at all. Name the contradiction
    // once instead of oscillating between its two halves. It exists only for a
    // process whose registry backend is that client route: the one host
    // process reads its registry from the local store it serves
    // (`WC_STORAGE_BACKEND=local`), and the URL the config keeps for other
    // clients of this host is then not the address this process reads through.
    let reads_through_client_route = matches!(
        crate::config::wc_storage_backend(),
        "stado" | "stado-object"
    );
    if !recovered && reads_through_client_route {
        let store_url = crate::config::wc_stado_storage_url();
        if let Ok(parsed) = url::Url::parse(store_url.trim()) {
            if let (Some(host), Some(port)) = (parsed.host_str(), parsed.port()) {
                let store_authority = format!("{host}:{port}");
                if let Some(adapter) = config
                    .adapters
                    .iter()
                    .find(|adapter| adapter.bind.trim() == store_authority)
                {
                    let detail = format!(
                        "this resolver is declared to serve {} for service {:?}, and the registry \
                         it must read first is configured at storage.stado.url = {}. One of the two \
                         has to move: either place the object API somewhere this resolver does not \
                         serve, or drop that adapter from the target's service_resolver policy. \
                         Retrying cannot resolve it.",
                        adapter.bind, adapter.service, store_url
                    );
                    publish(&PublishedState::failed(target, &detail));
                    return Err(CmdError::click(detail)
                        .stating(crate::primitives::failure::FailureCode::Config));
                }
            }
        }
    }

    let api = match bind_loopback(&config.api_bind).await {
        Ok(listener) => listener,
        Err(error) => {
            publish(&PublishedState::failed(target, &error.to_string()));
            return Err(error);
        }
    };
    let mut adapter_listeners = Vec::with_capacity(config.adapters.len());
    for adapter in &config.adapters {
        match bind_loopback(&adapter.bind).await {
            Ok(listener) => adapter_listeners.push((adapter.clone(), listener)),
            Err(error) => {
                publish(&PublishedState::failed(target, &error.to_string()));
                return Err(error);
            }
        }
    }

    // The markers are kept before the resolver says `serving`, so a host
    // whose resolver reports serving carries the directory's addresses.
    state.keep_markers().await;
    // Published before the first port is accepted on, so `resolver status`
    // answers `serving` for exactly the window the sockets are open.
    state.publish_serving().await;

    eprintln!(
        "stado resolver target={} api={} adapters={} refresh={}s max-stale={}s",
        target,
        config.api_bind,
        config.adapters.len(),
        config.refresh_seconds,
        config.max_stale_seconds
    );

    let mut tasks: JoinSet<Result<(), String>> = JoinSet::new();
    let (changes, mut changed) = tokio::sync::mpsc::unbounded_channel();
    let refresh_state = Arc::clone(&state);
    let refresh_seconds = config.refresh_seconds;
    tasks.spawn(async move { watch_registry(refresh_state, refresh_seconds, changes).await });
    let api_state = Arc::clone(&state);
    tasks.spawn(async move { serve_api(api, api_state).await });
    let mut listening = std::collections::HashMap::new();
    for (adapter, listener) in adapter_listeners {
        let adapter_state = Arc::clone(&state);
        let bind = adapter.bind.clone();
        let handle = tasks
            .spawn(async move { serve_adapter(listener, adapter.clone(), adapter_state).await });
        listening.insert(bind, handle);
    }
    let mut api_bind = config.api_bind.clone();

    let exit = loop {
        tokio::select! {
            Some(next) = changed.recv() => {
                if next.api_bind != api_bind {
                    break CmdError::click(format!(
                        "resolver API bind changed from {api_bind} to {}; restarting to rebind it",
                        next.api_bind
                    ))
                    .stating(crate::primitives::failure::FailureCode::Config);
                }
                if let Err(error) =
                    reconcile_adapters(&state, &next, &mut listening, &mut tasks).await
                {
                    break error;
                }
                api_bind = next.api_bind.clone();
                // A poisoned lock is a task that panicked while holding the
                // configuration: publishing `serving` over a configuration
                // that was never applied would vouch for routes nobody holds.
                match state.config.write() {
                    Ok(mut held) => *held = next,
                    Err(_) => {
                        break CmdError::unreachable(
                            "the resolver configuration lock is poisoned by a task that \
                             panicked; the new configuration was not applied",
                        )
                    }
                }
                state.publish_serving().await;
            }
            joined = tasks.join_next() => match joined {
                // An adapter this process retired on purpose.
                Some(Err(error)) if error.is_cancelled() => continue,
                // A data-plane task that ends takes its routes with it: every
                // consumer behind them sees the resolver down.
                Some(Ok(Ok(()))) => break CmdError::click("resolver task exited unexpectedly")
                    .stating(crate::primitives::failure::FailureCode::InfraDown),
                Some(Ok(Err(error))) => break CmdError::click(error)
                    .stating(crate::primitives::failure::FailureCode::InfraDown),
                Some(Err(error)) => break CmdError::click(format!("resolver task failed: {error}"))
                    .stating(crate::primitives::failure::FailureCode::InfraDown),
                // Nothing was started: the declaration this process serves
                // names no adapter and no route.
                None => break CmdError::declaration("resolver started no tasks"),
            },
        }
    };
    // The last thing this process says about itself. A task that died takes
    // the whole data plane with it, and leaving `serving` behind would make
    // `resolver status` vouch for a resolver that is gone.
    publish(&PublishedState::failed(target, &exit.to_string()));
    Err(exit)
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
/// A bind that fails ends the resolver with the holder named, as at startup.
async fn reconcile_adapters(
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

async fn bind_loopback(value: &str) -> Result<TcpListener, CmdError> {
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
    })
}

/// Reload the snapshot on the declared interval.
///
/// A failed refresh is published with the upstream's own error, and the next
/// read is the next declared tick. Adapters keep refusing with `service
/// directory cache is stale` meanwhile, which is the correct answer and no
/// longer an unexplained one.
async fn watch_registry(
    state: Arc<ResolverState>,
    refresh_seconds: u64,
    changes: tokio::sync::mpsc::UnboundedSender<service_resolution::ResolverConfig>,
) -> Result<(), String> {
    let refresh = Duration::from_secs(refresh_seconds);
    let mut interval = tokio::time::interval(refresh);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    interval.tick().await;
    let mut attempt = 0_u32;
    loop {
        interval.tick().await;
        match state.refresh().await {
            Ok(Some(next)) => {
                changes
                    .send(next)
                    .map_err(|_| "the resolver stopped taking configuration changes".to_string())?;
            }
            Ok(None) => {
                if attempt != 0 {
                    eprintln!("stado resolver refresh recovered after {attempt} failed attempts");
                    attempt = 0;
                }
                state.publish_serving().await;
            }
            Err(error) => {
                attempt = attempt.saturating_add(1);
                eprintln!(
                    "stado resolver refresh failed, attempt {attempt}, next on the declared \
                     {refresh_seconds}s interval: {error}"
                );
                // The listeners stay bound through a failed refresh.
                publish(
                    &PublishedState::backing_off(&state.local_target, attempt, &error, refresh)
                        .bound(state.published_adapters()),
                );
            }
        }
    }
}
