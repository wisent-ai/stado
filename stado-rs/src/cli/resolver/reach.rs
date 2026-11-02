//! Whether this host's object API could check its bearers against a vault
//! served on another host: the read the object API makes on every request
//! (`/v1/items/revision` of each namespace's bearer, as consumer `stado`),
//! carried over the native SSH channel this host's resolver would open to
//! that host, within the interval after which the resolver gives such a
//! request up.
//!
//! A vault move off the registry authority's host is committed only after
//! this answers. A move committed while the destination's vault answered its
//! own loopback probe but nothing over that channel took the authority's
//! store down with the commit, because every store request checks its bearer
//! against the vault the resolver now sends it to.

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::cli::resolver::authority::paths::target_ssh_paths;
use crate::cli::resolver::authority::tunnel::Tunnel;
use crate::cli::CmdError;
use crate::skarbiec::{Client, GrantMode};

/// The field of each namespace bearer item the object API checks.
const BEARER_FIELD: &str = "token";

/// Read every object API bearer's revision from the vault at `endpoint` on
/// `host`, through a native SSH channel to `host`, as consumer `stado` with
/// this host's own grant. The receipt names the bound, the session's time
/// and each read with its time and answer; `reached` is true only when every
/// read answered within the bound.
pub(crate) async fn vault_reach(host: &str, endpoint: &str) -> Result<Value, CmdError> {
    let url = url::Url::parse(endpoint)
        .map_err(|error| CmdError::usage(format!("--endpoint {endpoint} is not a URL: {error}")))?;
    let (Some(remote_host), Some(remote_port)) = (url.host_str(), url.port_or_known_default())
    else {
        return Err(CmdError::usage(format!(
            "--endpoint {endpoint} names no host and port to open a channel to"
        )));
    };
    let target = crate::cli::canonical_host(host).await?;
    let bound = crate::cli::resolver::bound_seconds(&crate::cli::resolver::Route::Host(&target.name))
        .map_err(CmdError::click)?
        .ok_or_else(|| {
            CmdError::declaration(
                "neither this host's resolver nor the registry declares a refresh interval, so \
                 the time within which the object API's vault read must answer cannot be read",
            )
        })?;
    let namespaces = crate::config::object_api_namespaces().map_err(|refusals| {
        CmdError::declaration(format!(
            "the object API's namespace policy does not load, so which bearers it checks \
             cannot be told: {}",
            refusals.join("; ")
        ))
    })?;
    let token_file = crate::config::skarbiec_token_file();
    let started = Instant::now();
    let mut reads = Vec::new();
    let namespace_count = namespaces.len();
    // Kept outside the bounded work, so a bound that expires still says how
    // far the work got: paths not chosen yet, no SSH session yet, or which
    // read was pending.
    let mut paths_seconds: Option<f64> = None;
    let mut path_names: Vec<String> = Vec::new();
    let mut session_seconds: Option<f64> = None;
    let mut attempts: Vec<Value> = Vec::new();
    let mut connecting: Option<String> = None;
    let mut pending: Option<String> = None;
    let work = async {
        // Choosing the paths reads the tailnet's status, which the resolver
        // pays too, so it is measured inside the bound.
        let paths = target_ssh_paths(&target);
        paths_seconds = Some(started.elapsed().as_secs_f64());
        path_names = paths
            .iter()
            .map(|path| format!("{} ({})", path.name, path.destination))
            .collect();
        // Each path is tried alone, in the order the resolver's tunnel tries
        // them, so the receipt says what each answered and how long it took.
        let mut opened = None;
        for path in &paths {
            let named = format!("{} ({})", path.name, path.destination);
            connecting = Some(named.clone());
            let attempt_started = Instant::now();
            let attempt = Tunnel::open(std::slice::from_ref(path)).await;
            let seconds = attempt_started.elapsed().as_secs_f64();
            match attempt {
                Ok(tunnel) => {
                    attempts.push(json!({"path": named, "seconds": seconds, "opened": true}));
                    opened = Some(tunnel);
                    break;
                }
                Err(error) => attempts.push(json!({
                    "path": named,
                    "seconds": seconds,
                    "opened": false,
                    "error": error,
                })),
            }
        }
        connecting = None;
        let tunnel = Arc::new(opened.ok_or_else(|| {
            format!(
                "no SSH session to {} opened over any of its paths: {}",
                target.name,
                attempts
                    .iter()
                    .map(|attempt| format!(
                        "{} after {:.3}s: {}",
                        attempt["path"].as_str().unwrap_or_default(),
                        attempt["seconds"].as_f64().unwrap_or_default(),
                        attempt["error"].as_str().unwrap_or_default()
                    ))
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        })?);
        session_seconds = Some(started.elapsed().as_secs_f64());
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, u16::default()))
            .await
            .map_err(|error| format!("no loopback port to carry the reads could be bound: {error}"))?;
        let local = listener
            .local_addr()
            .map_err(|error| format!("the bound loopback port could not be read back: {error}"))?;
        let carried = Arc::clone(&tunnel);
        let channel_host = remote_host.to_string();
        let carrier = tokio::spawn(async move {
            while let Ok((mut client, _)) = listener.accept().await {
                let tunnel = Arc::clone(&carried);
                let channel_host = channel_host.clone();
                tokio::spawn(async move {
                    if let Ok(mut upstream) = tunnel.connect(&channel_host, remote_port).await {
                        let _carried = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
                    }
                });
            }
        });
        let client = Client::direct(
            &format!("http://{local}"),
            "stado",
            token_file,
            GrantMode::RereadPerRequest,
        )
        .map_err(|error| format!("no Skarbiec client as consumer stado: {error}"))?;
        let mut failed = None;
        for (namespace, policy) in namespaces {
            pending = Some(format!("{namespace} ({})", policy.item()));
            let read_started = Instant::now();
            let answer = client.read_declared_revision(policy.item(), BEARER_FIELD).await;
            let seconds = read_started.elapsed().as_secs_f64();
            let (answered, detail) = match &answer {
                Ok(Some(version)) => (
                    true,
                    json!({"item_uid": version.item_uid, "revision": version.revision}),
                ),
                Ok(None) => (true, json!("absent")),
                Err(error) => (false, json!(error.to_string())),
            };
            reads.push(json!({
                "namespace": namespace,
                "item": policy.item(),
                "seconds": seconds,
                "answered": answered,
                "answer": detail,
            }));
            if !answered && failed.is_none() {
                failed = Some(format!("{namespace} ({}): {}", policy.item(), detail));
            }
        }
        pending = None;
        carrier.abort();
        Ok::<_, String>(failed)
    };
    let budget = Duration::from_secs(bound.seconds);
    let outcome = tokio::time::timeout(budget, work).await;
    let (reached, cause) = match outcome {
        Ok(Ok(None)) => (true, None),
        Ok(Ok(Some(failed))) => (
            false,
            Some(format!("the vault on {} did not answer the read of {failed}", target.name)),
        ),
        Ok(Err(cause)) => (false, Some(cause)),
        Err(_) => (
            false,
            Some(match (paths_seconds, session_seconds, &pending) {
                (None, _, _) => format!(
                    "choosing the SSH paths to {} (the tailnet status read) had not finished \
                     within {}s, the refresh interval {}; the resolver gives up the object \
                     API's own read after it",
                    target.name, bound.seconds, bound.declared
                ),
                (Some(chosen), None, _) => format!(
                    "the SSH paths to {} were chosen after {chosen:.3}s; {}{} was still \
                     connecting at {}s, the refresh interval {}; the resolver gives up the \
                     object API's own read after it",
                    target.name,
                    attempts
                        .iter()
                        .map(|attempt| format!(
                            "{} failed after {:.3}s: {}; ",
                            attempt["path"].as_str().unwrap_or_default(),
                            attempt["seconds"].as_f64().unwrap_or_default(),
                            attempt["error"].as_str().unwrap_or_default()
                        ))
                        .collect::<String>(),
                    connecting.as_deref().unwrap_or("no path"),
                    bound.seconds,
                    bound.declared
                ),
                (Some(_), Some(opened), pending) => format!(
                    "the SSH session to {} opened after {opened:.3}s, then {} of {} reads \
                     answered and the read of {} was still unanswered at {}s, the refresh \
                     interval {}; the resolver gives up the object API's own read after it",
                    target.name,
                    reads.len(),
                    namespace_count,
                    pending.as_deref().unwrap_or("no namespace"),
                    bound.seconds,
                    bound.declared
                ),
            }),
        ),
    };
    Ok(json!({
        "schema": "stado.vault-reach.v1",
        "host": target.name,
        "endpoint": endpoint,
        "bound_seconds": bound.seconds,
        "bound_declared": bound.declared,
        "paths": path_names,
        "paths_seconds": paths_seconds,
        "session_seconds": session_seconds,
        "attempts": attempts,
        "seconds": started.elapsed().as_secs_f64(),
        "reads": reads,
        "reached": reached,
        "cause": cause,
    }))
}
