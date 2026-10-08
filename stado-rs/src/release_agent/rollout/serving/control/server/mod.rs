use std::collections::BTreeMap;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;

use futures::FutureExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;

use super::socket::{ControlSocketError, Prepared};
use super::{Action, OwnedProxy, OwnedTransaction, Request, Response, SCHEMA};
use crate::release_agent::rollout::serving::proxy::{forward, ProxyState};

mod workers;

use workers::Worker;

/// What one request produced.
enum Reply {
    Proxy(Option<OwnedProxy>),
    Transaction(Option<OwnedTransaction>),
}

struct Route {
    identity: OwnedProxy,
    task: JoinHandle<()>,
    stop: oneshot::Sender<()>,
}

struct Owner {
    routes: Mutex<BTreeMap<PathBuf, Route>>,
    transactions: Mutex<BTreeMap<String, Worker>>,
    failures: mpsc::UnboundedSender<String>,
    uid: u32,
}

pub(crate) async fn serve(prepared: Prepared) -> Result<(), ControlSocketError> {
    let Prepared { listener, guard } = prepared;
    let _guard = guard;
    let listener = UnixListener::from_std(listener).map_err(|error| ControlSocketError::Io {
        context: "cannot attach proxy control listener".to_string(),
        error,
    })?;
    let (sender, mut failures) = mpsc::unbounded_channel();
    let owner = Arc::new(Owner {
        routes: Mutex::new(BTreeMap::new()),
        transactions: Mutex::new(BTreeMap::new()),
        failures: sender,
        uid: nix::unistd::geteuid().as_raw(),
    });
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted.map_err(|error| ControlSocketError::Io {
                    context: "proxy control accept failed".to_string(),
                    error,
                })?;
                let owner = Arc::clone(&owner);
                tokio::spawn(async move {
                    if let Err(error) = respond(stream, &owner).await {
                        eprintln!("stado release proxy control: {error}");
                    }
                });
            }
            Some(failure) = failures.recv() => return Err(ControlSocketError::RouteFailed(failure)),
        }
    }
}

async fn respond(mut stream: UnixStream, owner: &Owner) -> Result<(), String> {
    let outcome = read_and_apply(&mut stream, owner).await;
    let (proxy, transaction, error) = match outcome {
        Ok(Reply::Proxy(proxy)) => (proxy, None, None),
        Ok(Reply::Transaction(transaction)) => (None, transaction, None),
        Err(error) => (None, None, Some(error)),
    };
    let response = Response {
        schema_version: SCHEMA,
        pid: std::process::id() as i32,
        proxy,
        transaction,
        error,
    };
    let bytes = serde_json::to_vec(&response)
        .map_err(|error| format!("cannot encode proxy control response: {error}"))?;
    stream
        .write_all(&bytes)
        .await
        .map_err(|error| format!("cannot send proxy control response: {error}"))
}

async fn read_and_apply(stream: &mut UnixStream, owner: &Owner) -> Result<Reply, String> {
    let peer = stream
        .peer_cred()
        .map_err(|error| format!("cannot read native proxy caller credentials: {error}"))?;
    if peer.uid() != owner.uid && peer.uid() != 0 {
        return Err(format!(
            "proxy control refuses uid {}; owner uid is {}",
            peer.uid(),
            owner.uid
        ));
    }
    // Only the owner (or root) gets here, so the request is read whole.
    let mut bytes = Vec::new();
    (*stream)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| format!("cannot read proxy control request: {error}"))?;
    let request: Request = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid proxy control request: {error}"))?;
    if request.schema_version != SCHEMA {
        return Err(format!(
            "unsupported proxy control schema {}",
            request.schema_version
        ));
    }
    match request.action {
        Action::AdoptTransaction {
            transaction,
            argv,
            env,
            working_directory,
            log,
        } => workers::adopt(
            &owner.transactions,
            transaction,
            argv,
            env,
            working_directory,
            log,
        )
        .await
        .map(|owned| Reply::Transaction(Some(owned))),
        Action::InspectTransaction { transaction } => {
            let mut transactions = owner.transactions.lock().await;
            Ok(Reply::Transaction(
                transactions
                    .get_mut(&transaction)
                    .map(|worker| worker.owned(&transaction)),
            ))
        }
        Action::Retire { state } => retire(&state, owner).await.map(|()| Reply::Proxy(None)),
        action => apply(action, owner).await.map(Reply::Proxy),
    }
}

/// Stop the route `state` owns, whatever its bind.
async fn retire(state: &std::path::Path, owner: &Owner) -> Result<(), String> {
    if !state.is_absolute() {
        return Err("proxy retirement requires an absolute state path".to_string());
    }
    let Some(route) = owner.routes.lock().await.remove(state) else {
        return Ok(());
    };
    let bind = route.identity.bind;
    route
        .stop
        .send(())
        .map_err(|_| format!("proxy {bind} stopped before acknowledging retirement"))?;
    route
        .task
        .await
        .map_err(|error| format!("retiring proxy {bind} failed: {error}"))?;
    eprintln!(
        "stado release proxy retired: pid={} bind={bind} state={}",
        std::process::id(),
        state.display()
    );
    Ok(())
}

async fn apply(action: Action, owner: &Owner) -> Result<Option<OwnedProxy>, String> {
    let (state, bind) = match &action {
        Action::Ensure { state, bind }
        | Action::Inspect { state, bind }
        | Action::Stop { state, bind } => (state, *bind),
        Action::Retire { .. }
        | Action::AdoptTransaction { .. }
        | Action::InspectTransaction { .. } => {
            return Err("a transaction request reached the proxy table".to_string())
        }
    };
    if !state.is_absolute() || !bind.ip().is_loopback() || bind.port() == 0 {
        return Err(
            "proxy control requires an absolute state path and nonzero loopback bind".to_string(),
        );
    }
    let mut routes = owner.routes.lock().await;
    if let Some(route) = routes.get(state) {
        if route.identity.bind != bind {
            return Err(format!(
                "{} already owns {}, not {bind}",
                state.display(),
                route.identity.bind
            ));
        }
        if route.task.is_finished() {
            return Err(format!(
                "proxy {} for {} terminated unexpectedly",
                bind,
                state.display()
            ));
        }
    }
    match action {
        Action::Inspect { state, .. } => Ok(routes.get(&state).map(|route| route.identity.clone())),
        Action::Stop { state, .. } => {
            if let Some(route) = routes.remove(&state) {
                route
                    .stop
                    .send(())
                    .map_err(|_| format!("proxy {bind} stopped before acknowledging shutdown"))?;
                route
                    .task
                    .await
                    .map_err(|error| format!("stopping proxy {bind} failed: {error}"))?;
                eprintln!(
                    "stado release proxy stopped: pid={} bind={bind} state={}",
                    std::process::id(),
                    state.display()
                );
            }
            Ok(None)
        }
        Action::Ensure { state, .. } => {
            if let Some(route) = routes.get(&state) {
                return Ok(Some(route.identity.clone()));
            }
            let bytes = tokio::fs::read(&state).await.map_err(|error| {
                format!("cannot read proxy target {}: {error}", state.display())
            })?;
            let configured: ProxyState = serde_json::from_slice(&bytes)
                .map_err(|error| format!("invalid proxy target {}: {error}", state.display()))?;
            let upstream: std::net::SocketAddr = configured
                .upstream
                .parse()
                .map_err(|error| format!("invalid proxy upstream: {error}"))?;
            if !upstream.ip().is_loopback() {
                return Err("proxy upstream must be loopback".to_string());
            }
            let listener = TcpListener::bind(bind)
                .await
                .map_err(|error| format!("cannot bind release proxy {bind}: {error}"))?;
            let identity = OwnedProxy {
                state: state.clone(),
                bind: listener
                    .local_addr()
                    .map_err(|error| format!("cannot inspect bound proxy: {error}"))?,
            };
            let path = state.clone();
            let failures = owner.failures.clone();
            let (stop, stopped) = oneshot::channel();
            let task = tokio::spawn(async move {
                let result = AssertUnwindSafe(forward(listener, path, stopped))
                    .catch_unwind()
                    .await;
                let detail = match result {
                    Ok(Err(error)) => error,
                    Ok(Ok(())) => return,
                    Err(payload) => {
                        let message = payload
                            .downcast_ref::<String>()
                            .map(String::as_str)
                            .or_else(|| payload.downcast_ref::<&str>().copied())
                            .unwrap_or("non-text panic payload");
                        format!("forwarding loop panicked: {message}")
                    }
                };
                let _ = failures.send(format!("release proxy {bind} stopped: {detail}"));
            });
            routes.insert(
                state.clone(),
                Route {
                    identity: identity.clone(),
                    task,
                    stop,
                },
            );
            eprintln!(
                "stado release proxy listening: pid={} bind={bind} state={}",
                std::process::id(),
                state.display()
            );
            Ok(Some(identity))
        }
        Action::Retire { .. }
        | Action::AdoptTransaction { .. }
        | Action::InspectTransaction { .. } => {
            Err("a transaction request reached the proxy table".to_string())
        }
    }
}
