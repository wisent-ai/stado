//! A finite release command delegates listeners to the persistent host process.
//! The owner-only Unix socket preserves local OS identity; no proxy daemon is
//! spawned and no network management endpoint is exposed.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

mod client;
mod server;
mod socket;

pub(crate) use server::serve;
pub(crate) use socket::prepare;

const SCHEMA: u32 = 1;
const FRAME_LIMIT: u64 = 64 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    schema_version: u32,
    action: Action,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Ensure { state: PathBuf, bind: SocketAddr },
    Inspect { state: PathBuf, bind: SocketAddr },
    Stop { state: PathBuf, bind: SocketAddr },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnedProxy {
    state: PathBuf,
    bind: SocketAddr,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    schema_version: u32,
    pid: i32,
    proxy: Option<OwnedProxy>,
    error: Option<String>,
}

fn socket_path(home: Option<&str>) -> Result<PathBuf, String> {
    let path = if let Some(path) = std::env::var_os("STADO_RELEASE_PROXY_SOCKET") {
        PathBuf::from(path)
    } else {
        let home = home
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .ok_or_else(|| "release proxy owner has no HOME".to_string())?;
        home.join(".stado/release-proxy.sock")
    };
    if !path.is_absolute() {
        return Err(format!(
            "release proxy control socket must be absolute: {}",
            path.display()
        ));
    }
    Ok(path)
}

fn coordinates(state: &Path, bind: &str) -> Result<(PathBuf, SocketAddr), String> {
    let state = std::path::absolute(state)
        .map_err(|error| format!("cannot resolve proxy state {}: {error}", state.display()))?;
    let bind: SocketAddr = bind
        .parse()
        .map_err(|error| format!("invalid release proxy bind {bind:?}: {error}"))?;
    if !bind.ip().is_loopback() || bind.port() == 0 {
        return Err("release proxy bind must name a nonzero loopback port".to_string());
    }
    Ok((state, bind))
}

pub(crate) async fn ensure(home: Option<&str>, state: &Path, bind: &str) -> Result<i32, String> {
    let (state, bind) = coordinates(state, bind)?;
    let response = client::exchange(home, Action::Ensure { state, bind })
        .await?
        .ok_or_else(|| "release proxy owner disappeared during ensure".to_string())?;
    if response.proxy.is_none() {
        return Err(
            "release proxy owner acknowledged ensure without owning its listener".to_string(),
        );
    }
    Ok(response.pid)
}

pub(crate) async fn inspect(
    home: Option<&str>,
    state: &Path,
    bind: &str,
) -> Result<Option<i32>, String> {
    let (state, bind) = coordinates(state, bind)?;
    Ok(client::exchange(home, Action::Inspect { state, bind })
        .await?
        .and_then(|response| response.proxy.map(|_| response.pid)))
}

pub(crate) async fn require_owner(
    home: Option<&str>,
    state: &Path,
    bind: &str,
) -> Result<i32, String> {
    let (state, bind) = coordinates(state, bind)?;
    client::exchange(home, Action::Inspect { state, bind })
        .await?
        .map(|response| response.pid)
        .ok_or_else(|| {
            "release proxy owner is unavailable; the host must run stado serve before a rollout"
                .to_string()
        })
}

pub(crate) async fn stop(home: Option<&str>, state: &Path, bind: &str) -> Result<(), String> {
    let (state, bind) = coordinates(state, bind)?;
    let response = client::exchange(home, Action::Stop { state, bind })
        .await?
        .ok_or_else(|| "release proxy owner disappeared during stop".to_string())?;
    if response.proxy.is_some() {
        return Err(
            "release proxy owner acknowledged stop but still owns the listener".to_string(),
        );
    }
    Ok(())
}
