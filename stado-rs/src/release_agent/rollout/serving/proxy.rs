//! The stable loopback proxy: the target document it forwards by, the process
//! that owns the bind, and the forwarding loop itself.

use std::net::SocketAddr;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::io::copy_bidirectional;
use tokio::net::{TcpListener, TcpStream};

use crate::release_agent::state::document::{atomic_json, proxy_state_path};
use crate::release_agent::state::evidence::release_log;
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

pub(crate) fn start_proxy(
    target: &ReleaseTargetPolicy,
    serving: &BlueGreenServing,
    product: &str,
    generation: u64,
    port: u16,
) -> Result<i32, String> {
    write_proxy_target(target, product, generation, port)?;
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot resolve Stado executable: {error}"))?;
    let stdout = release_log(target, product, "proxy", "out")?;
    let stderr = release_log(target, product, "proxy", "err")?;
    let child = Command::new(executable)
        .args([
            "release",
            "proxy",
            "--state",
            &proxy_state_path(target, product).display().to_string(),
            "--bind",
            &serving.stable_bind,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|error| format!("cannot start stable release proxy: {error}"))?;
    Ok(child.id() as i32)
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
    let bind: SocketAddr = bind
        .parse()
        .map_err(|_| "release proxy bind is not a socket address".to_string())?;
    if !bind.ip().is_loopback() {
        return Err("release proxy bind must be loopback".to_string());
    }
    let listener = TcpListener::bind(bind)
        .await
        .map_err(|error| format!("cannot bind release proxy {bind}: {error}"))?;
    loop {
        let (mut client, _) = listener
            .accept()
            .await
            .map_err(|error| format!("release proxy accept failed: {error}"))?;
        let state_path = state_path.to_path_buf();
        tokio::spawn(async move {
            let result = async {
                let state: ProxyState = serde_json::from_slice(
                    &tokio::fs::read(&state_path)
                        .await
                        .map_err(|error| format!("cannot read proxy state: {error}"))?,
                )
                .map_err(|error| format!("invalid proxy state: {error}"))?;
                let upstream: SocketAddr = state
                    .upstream
                    .parse()
                    .map_err(|_| "proxy upstream is not a socket address".to_string())?;
                if !upstream.ip().is_loopback() {
                    return Err("proxy upstream must be loopback".to_string());
                }
                let mut server = TcpStream::connect(upstream)
                    .await
                    .map_err(|error| format!("proxy upstream connect failed: {error}"))?;
                copy_bidirectional(&mut client, &mut server)
                    .await
                    .map_err(|error| format!("release proxy failed: {error}"))?;
                Ok::<(), String>(())
            }
            .await;
            if let Err(error) = result {
                eprintln!("stado release proxy connection failed: {error}");
            }
        });
    }
}
