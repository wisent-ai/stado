//! Stable loopback forwarding owned by the host service. Finite release commands
//! change targets and request listeners; they never leave a proxy process behind.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::io::copy_bidirectional;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinSet;

use crate::release_agent::state::document::{atomic_json, proxy_state_path};
use crate::release_control::{BlueGreenServing, ReleaseTargetPolicy};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProxyState {
    pub(crate) generation: u64,
    pub(crate) upstream: String,
    updated_at: DateTime<Utc>,
}

pub(crate) fn write_proxy_target(
    target: &ReleaseTargetPolicy,
    product: &str,
    generation: u64,
    port: u16,
) -> Result<(), String> {
    atomic_json(
        &proxy_state_path(target, product),
        &ProxyState {
            generation,
            upstream: format!("127.0.0.1:{port}"),
            updated_at: Utc::now(),
        },
    )
}

pub(crate) async fn start_proxy(
    target: &ReleaseTargetPolicy,
    serving: &BlueGreenServing,
    product: &str,
    generation: u64,
    port: u16,
) -> Result<i32, String> {
    write_proxy_target(target, product, generation, port)?;
    super::control::ensure(
        Some(&target.home),
        &proxy_state_path(target, product),
        &serving.stable_bind,
    )
    .await
}

/// The port the proxy currently forwards to, read from its own target file.
///
/// When the state file has lost its `active` record -- interrupted rollouts and an
/// orphaned reconciler both did that this week -- the proxy's target is the only
/// truthful statement of which port carries traffic.
pub(crate) fn proxy_upstream_port(target: &ReleaseTargetPolicy, product: &str) -> Option<u16> {
    let raw = std::fs::read(proxy_state_path(target, product)).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    value
        .get("upstream")?
        .as_str()?
        .rsplit(':')
        .next()?
        .parse()
        .ok()
}

pub(crate) async fn stable_bind_ready(serving: &BlueGreenServing) -> bool {
    let url = format!("http://{}{}", serving.stable_bind, serving.readiness_path);
    reqwest::Client::new()
        .get(url)
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}

pub async fn proxy(state_path: &Path, bind: &str) -> Result<(), String> {
    let pid = super::control::ensure(None, state_path, bind).await?;
    eprintln!(
        "stado release proxy owned by pid={pid} bind={bind} state={}",
        state_path.display()
    );
    Ok(())
}

pub(super) async fn forward(
    listener: TcpListener,
    state_path: PathBuf,
    mut stopped: tokio::sync::oneshot::Receiver<()>,
) -> Result<(), String> {
    let state_path = Arc::new(state_path);
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            requested = &mut stopped => {
                requested.map_err(|_| "release proxy owner dropped its shutdown channel".to_string())?;
                drop(listener);
                connections.shutdown().await;
                return Ok(());
            }
            accepted = listener.accept() => {
                let (mut client, _) = accepted
                    .map_err(|error| format!("release proxy accept failed: {error}"))?;
                let state_path = Arc::clone(&state_path);
                connections.spawn(async move {
                    let result = async {
                        let state: ProxyState = serde_json::from_slice(
                            &tokio::fs::read(state_path.as_path()).await
                                .map_err(|error| format!("cannot read proxy state: {error}"))?,
                        )
                        .map_err(|error| format!("invalid proxy state: {error}"))?;
                        let upstream: SocketAddr = state.upstream.parse()
                            .map_err(|_| "proxy upstream is not a socket address".to_string())?;
                        if !upstream.ip().is_loopback() {
                            return Err("proxy upstream must be loopback".to_string());
                        }
                        let mut server = TcpStream::connect(upstream).await
                            .map_err(|error| format!("proxy upstream connect failed: {error}"))?;
                        copy_bidirectional(&mut client, &mut server).await
                            .map_err(|error| format!("release proxy failed: {error}"))?;
                        Ok::<(), String>(())
                    }.await;
                    if let Err(error) = result {
                        eprintln!("stado release proxy connection failed: {error}");
                    }
                });
            }
            Some(result) = connections.join_next(), if !connections.is_empty() => {
                if let Err(error) = result {
                    eprintln!("stado release proxy connection task failed: {error}");
                }
            }
        }
    }
}
