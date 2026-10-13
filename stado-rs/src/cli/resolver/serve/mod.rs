use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use tokio::task::JoinSet;

use crate::service_resolution;
use crate::targets::RegistryStore;

use crate::cli::CmdError;

mod adapters;
mod api;
mod proxy;
mod startup;
mod state;

use adapters::{bind_adapters, bind_loopback, reconcile_adapters, self_served_store};

use crate::cli::resolver::directory::source::SnapshotSource;
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
    crate::cli::resolver::report::waiting::start();
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
    // A host whose registry authority is another target reads every snapshot
    // from that authority over SSH; only the authority itself reads the
    // registry through its local store.
    let reads_local_store = matches!(source, SnapshotSource::Local(_));
    // A resolver that must serve the very address it reads the registry
    // through cannot bind that one adapter in either order (see
    // [`self_served_store`]). It exists only for a process whose registry is
    // read through that client route: the one host process reads its
    // registry from the local store it serves (`WC_STORAGE_BACKEND=local`),
    // and a host whose authority is another target reads its snapshots from
    // that authority, so a store URL this resolver serves is then the route
    // it gives its own clients, not one it depends on.
    let refuses_store_adapter = !recovered
        && reads_local_store
        && matches!(
            crate::config::wc_storage_backend(),
            "stado" | "stado-object"
        );
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
        adapters: std::sync::RwLock::new(Vec::new()),
        unserved: std::sync::RwLock::new(Vec::new()),
        refuses_store_adapter,
        config: std::sync::RwLock::new(config.clone()),
        tunnels: tokio::sync::Mutex::new(std::collections::HashMap::new()),
        // Startup has not written a copy yet; the first `refresh` sets this
        // either way.
        last_good_refusal: RwLock::new(None),
    });

    let api = match bind_loopback(&config.api_bind).await {
        Ok(listener) => listener,
        Err(error) => {
            publish(&PublishedState::failed(target, &error.to_string()));
            return Err(error);
        }
    };
    // Every route this process can serve is served; one it cannot (the store
    // it reads through, a port something else holds) is published unserved
    // with its own reason instead of taking every other route down with it.
    let refused = self_served_store(&config.adapters, refuses_store_adapter);
    let (adapter_listeners, unserved) = bind_adapters(&config.adapters, refused).await;
    state.hold(
        adapter_listeners
            .iter()
            .map(|(adapter, _)| adapter.clone())
            .collect(),
        unserved,
    );

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
