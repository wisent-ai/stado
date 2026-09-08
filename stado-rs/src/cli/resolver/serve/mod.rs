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

use crate::cli::resolver::authority::drop_stale_ssh_sockets;
use crate::cli::resolver::report::published::backoff_delay;
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
    drop_stale_ssh_sockets();
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
            return Err(CmdError::click(detail));
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
        adapters: config.adapters.clone(),
        config: config.clone(),
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
    // once instead of oscillating between its two halves.
    if !recovered {
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
                    return Err(CmdError::click(detail));
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

    let mut tasks = JoinSet::new();
    let refresh_state = Arc::clone(&state);
    tasks.spawn(async move { watch_registry(refresh_state, config.refresh_seconds).await });
    let api_state = Arc::clone(&state);
    tasks.spawn(async move { serve_api(api, api_state).await });
    for (adapter, listener) in adapter_listeners {
        let adapter_state = Arc::clone(&state);
        tasks.spawn(async move { serve_adapter(listener, adapter, adapter_state).await });
    }

    let exit = match tasks.join_next().await {
        Some(Ok(Ok(()))) => CmdError::click("resolver task exited unexpectedly"),
        Some(Ok(Err(error))) => CmdError::click(error),
        Some(Err(error)) => CmdError::click(format!("resolver task failed: {error}")),
        None => CmdError::click("resolver started no tasks"),
    };
    // The last thing this process says about itself. A task that died takes
    // the whole data plane with it, and leaving `serving` behind would make
    // `resolver status` vouch for a resolver that is gone.
    publish(&PublishedState::failed(target, &exit.to_string()));
    Err(exit)
}

async fn bind_loopback(value: &str) -> Result<TcpListener, CmdError> {
    let address: SocketAddr = value
        .parse()
        .map_err(|_| CmdError::click(format!("invalid resolver bind {value:?}")))?;
    if !address.ip().is_loopback() {
        return Err(CmdError::click(format!(
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

/// Reload the snapshot on the declared interval, backing off when the
/// upstream will not answer.
///
/// The plain interval hammered a dead authority at `refresh_seconds`: on
/// 2026-08-19 that produced seven identical `registry authority exited with
/// exit status: 255: ssh: connect to host 100.120.25.24 port 22: Operation
/// timed out` lines, each costing a ten second ssh connect, and told nobody
/// anything the first had not. The reason is published once per attempt now,
/// and the wait between attempts grows to [`BACKOFF_CAP`]. Adapters keep
/// refusing with `service directory cache is stale` while this loop backs
/// off, which is the correct answer and no longer an unexplained one.
async fn watch_registry(state: Arc<ResolverState>, refresh_seconds: u64) -> Result<(), String> {
    let mut interval = tokio::time::interval(Duration::from_secs(refresh_seconds));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    interval.tick().await;
    let mut attempt = 0_u32;
    loop {
        interval.tick().await;
        match state.refresh().await {
            Ok(true) => {
                return Err(
                    "resolver configuration changed; restarting to rebind listeners".to_string(),
                )
            }
            Ok(false) => {
                if attempt != 0 {
                    eprintln!("stado resolver refresh recovered after {attempt} failed attempts");
                    attempt = 0;
                }
                state.publish_serving().await;
            }
            Err(error) => {
                attempt = attempt.saturating_add(1);
                let delay = backoff_delay(attempt);
                eprintln!(
                    "stado resolver refresh failed, attempt {attempt}, next in {}s: {error}",
                    delay.as_secs()
                );
                publish(&PublishedState::backing_off(
                    &state.local_target,
                    attempt,
                    &error,
                    delay,
                ));
                tokio::time::sleep(delay).await;
            }
        }
    }
}
