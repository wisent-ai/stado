//! A finite command delegates long-lived work to the persistent host process.
//! The owner-only Unix socket preserves local OS identity; no proxy daemon is
//! spawned and no network management endpoint is exposed.
//!
//! Two kinds of work are delegated: a release proxy listener, and a storage
//! root transaction's worker, which the host process runs as its own child
//! so that no unit of the transaction's own is ever installed.

use std::collections::BTreeMap;
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

mod client;
mod error;
mod server;
mod socket;
mod transactions;

pub(crate) use error::ControlClientError;
pub(crate) use server::serve;
pub(crate) use socket::{prepare, ControlSocketError};
pub(crate) use transactions::{
    adopt_transaction_blocking, inspect_transaction_blocking, OwnedTransaction, TransactionRequest,
};

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
    Ensure {
        state: PathBuf,
        bind: SocketAddr,
    },
    Inspect {
        state: PathBuf,
        bind: SocketAddr,
    },
    Stop {
        state: PathBuf,
        bind: SocketAddr,
    },
    /// Stop whatever proxy the state file owns, on whichever bind: the
    /// product's policy no longer names this host, so nothing states the bind.
    Retire {
        state: PathBuf,
    },
    /// Run a storage root transaction's worker as a child of this process.
    AdoptTransaction {
        transaction: String,
        argv: Vec<String>,
        env: BTreeMap<String, String>,
        working_directory: PathBuf,
        log: PathBuf,
    },
    /// Whether this process runs the transaction's worker, and as which pid.
    InspectTransaction {
        transaction: String,
    },
}

impl Action {
    /// A read that is answered `None` when no host process is there to ask.
    fn is_inspection(&self) -> bool {
        matches!(self, Self::Inspect { .. } | Self::InspectTransaction { .. })
    }
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
    #[serde(default)]
    transaction: Option<OwnedTransaction>,
    error: Option<String>,
}

/// Run `future` from a synchronous caller inside this program's runtime.
fn block_on<F: Future>(future: F) -> F::Output {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| handle.block_on(future)),
        Err(_) => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime for the host process control client")
            .block_on(future),
    }
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

fn coordinates(state: &Path, bind: &str) -> Result<(PathBuf, SocketAddr), ControlClientError> {
    let state = std::path::absolute(state).map_err(ControlClientError::io(format!(
        "cannot resolve proxy state {}",
        state.display()
    )))?;
    let bind: SocketAddr = bind.parse().map_err(|error| {
        ControlClientError::Config(format!("invalid release proxy bind {bind:?}: {error}"))
    })?;
    if !bind.ip().is_loopback() || bind.port() == 0 {
        return Err(ControlClientError::Config(
            "release proxy bind must name a nonzero loopback port".to_string(),
        ));
    }
    Ok((state, bind))
}

pub(crate) async fn ensure(
    home: Option<&str>,
    state: &Path,
    bind: &str,
) -> Result<i32, ControlClientError> {
    let (state, bind) = coordinates(state, bind)?;
    let response = client::exchange(home, Action::Ensure { state, bind })
        .await?
        .ok_or_else(|| {
            ControlClientError::Unavailable(
                "release proxy owner disappeared during ensure".to_string(),
            )
        })?;
    if response.proxy.is_none() {
        return Err(ControlClientError::Refused(
            "release proxy owner acknowledged ensure without owning its listener".to_string(),
        ));
    }
    Ok(response.pid)
}

pub(crate) async fn inspect(
    home: Option<&str>,
    state: &Path,
    bind: &str,
) -> Result<Option<i32>, ControlClientError> {
    let (state, bind) = coordinates(state, bind)?;
    Ok(client::exchange(home, Action::Inspect { state, bind })
        .await?
        .and_then(|response| response.proxy.map(|_| response.pid)))
}

pub(crate) async fn require_owner(
    home: Option<&str>,
    state: &Path,
    bind: &str,
) -> Result<i32, ControlClientError> {
    let (state, bind) = coordinates(state, bind)?;
    client::exchange(home, Action::Inspect { state, bind })
        .await?
        .map(|response| response.pid)
        .ok_or_else(|| {
            ControlClientError::Unavailable(
                "release proxy owner is unavailable; the host must run stado serve before a rollout"
                    .to_string(),
            )
        })
}

pub(crate) async fn stop(
    home: Option<&str>,
    state: &Path,
    bind: &str,
) -> Result<(), ControlClientError> {
    let (state, bind) = coordinates(state, bind)?;
    let response = client::exchange(home, Action::Stop { state, bind })
        .await?
        .ok_or_else(|| {
            ControlClientError::Unavailable(
                "release proxy owner disappeared during stop".to_string(),
            )
        })?;
    if response.proxy.is_some() {
        return Err(ControlClientError::Refused(
            "release proxy owner acknowledged stop but still owns the listener".to_string(),
        ));
    }
    Ok(())
}

/// Stop the proxy `state` owns, whatever its bind, for a product whose policy
/// no longer targets this host. A state that owns no proxy is not an error.
pub(crate) async fn retire(home: Option<&str>, state: &Path) -> Result<(), ControlClientError> {
    if !state.is_absolute() {
        return Err(ControlClientError::Config(format!(
            "proxy retirement requires an absolute state path, not {}",
            state.display()
        )));
    }
    let response = client::exchange(
        home,
        Action::Retire {
            state: state.to_path_buf(),
        },
    )
    .await?;
    if response.is_some_and(|response| response.proxy.is_some()) {
        return Err(ControlClientError::Refused(
            "release proxy owner acknowledged retirement but still owns the listener".to_string(),
        ));
    }
    Ok(())
}
