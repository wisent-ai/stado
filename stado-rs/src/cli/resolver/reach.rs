//! Whether this host's object API could check its bearers against a vault
//! served on another host: the read the object API makes on every request
//! (`/v1/items/revision` of each namespace's bearer, as consumer `stado`),
//! carried over the native SSH channel this host's resolver would open to
//! that host, each read within the interval after which the resolver gives
//! such a request up, less the time the session took to open.
//!
//! The bound is one request's, not the sum of all reads: the object API makes
//! one read per request and the resolver gives each request up on its own.
//! Bounding the reads together refused a channel that answered every read in
//! a fraction of a second, only because there were many namespaces.
//!
//! A vault move off the registry authority's host is committed only after
//! this answers. A move committed while the destination's vault answered its
//! own loopback probe but nothing over that channel took the authority's
//! store down with the commit, because every store request checks its bearer
//! against the vault the resolver now sends it to.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::cli::resolver::authority::paths::target_ssh_paths;
use crate::cli::resolver::authority::tunnel::Tunnel;
use crate::cli::CmdError;
use crate::config::ObjectApiNamespace;
use crate::skarbiec::{Client, GrantMode};

/// The field of each namespace bearer item the object API checks.
const BEARER_FIELD: &str = "token";

/// Read every object API bearer's revision from the vault at `endpoint` on
/// `host`, through a native SSH channel to `host`, as consumer `stado` with
/// this host's own grant. The session must open within the bound, and each
/// read must answer within the bound less the session's time, the request
/// that also opens the session. The receipt names the bound, the session's
/// time, the per-read bound and each read with its time and answer;
/// `reached` is true only when every read answered within it.
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
    let budget = Duration::from_secs(bound.seconds);
    let mut reads = Vec::new();
    let namespace_count = namespaces.len();
    // Kept outside the bounded work, so a bound that expires still says how
    // far the work got: paths not chosen yet or no SSH session yet.
    let mut paths_seconds: Option<f64> = None;
    let mut path_names: Vec<String> = Vec::new();
    let mut session_seconds: Option<f64> = None;
    let mut attempts: Vec<Value> = Vec::new();
    let mut connecting: Option<String> = None;
    let mut read_bound_seconds: Option<f64> = None;
    let opening = async {
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
        let tunnel = opened.ok_or_else(|| {
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
        })?;
        session_seconds = Some(started.elapsed().as_secs_f64());
        Ok::<_, String>(Arc::new(tunnel))
    };
    let opened = tokio::time::timeout(budget, opening).await;
    let (reached, cause) = match opened {
        Err(_) => (
            false,
            Some(match paths_seconds {
                None => format!(
                    "choosing the SSH paths to {} (the tailnet status read) had not finished \
                     within {}s, the refresh interval {}; the resolver gives up the object \
                     API's own read after it",
                    target.name, bound.seconds, bound.declared
                ),
                Some(chosen) => format!(
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
            }),
        ),
        Ok(Err(cause)) => (false, Some(cause)),
        Ok(Ok(tunnel)) => {
            // The object API makes one read per request and the resolver gives
            // each request up at the bound on its own, so each read is bounded
            // alone, not the reads together. Any request may be the one that
            // opens the session, so each read gets the bound less the time
            // the session took to open.
            let opened_after = session_seconds.unwrap_or_default();
            let per_read = budget.saturating_sub(Duration::from_secs_f64(opened_after));
            read_bound_seconds = Some(per_read.as_secs_f64());
            match read_bearers(
                tunnel,
                remote_host,
                remote_port,
                token_file,
                namespaces,
                per_read,
                &mut reads,
            )
            .await
            {
                Ok(Read::Answered) => (true, None),
                Ok(Read::Refused(failed)) => (
                    false,
                    Some(format!("the vault on {} did not answer the read of {failed}", target.name)),
                ),
                Ok(Read::Unanswered(pending)) => (
                    false,
                    Some(format!(
                        "the SSH session to {} opened after {opened_after:.3}s, then {} of {} \
                         reads answered and the read of {pending} was still unanswered {:.3}s \
                         later: a request that opens the session and makes that read passes \
                         {}s, the refresh interval {}; the resolver gives up the object API's \
                         own read after it",
                        target.name,
                        reads.len(),
                        namespace_count,
                        per_read.as_secs_f64(),
                        bound.seconds,
                        bound.declared
                    )),
                ),
                Err(cause) => (false, Some(cause)),
            }
        }
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
        "read_bound_seconds": read_bound_seconds,
        "attempts": attempts,
        "seconds": started.elapsed().as_secs_f64(),
        "reads": reads,
        "reached": reached,
        "cause": cause,
    }))
}

/// How the bearer reads over an opened session ended.
enum Read {
    /// Every read answered within its bound.
    Answered,
    /// A read answered with a refusal or an error; the namespace, item and
    /// answer of the first one.
    Refused(String),
    /// A read was still unanswered at its bound; the namespace and item. The
    /// reads after it are not made: over a channel that held one read past
    /// the bound, each would only hold the bound again.
    Unanswered(String),
}

/// Read every namespace bearer's revision through `tunnel`, each read bounded
/// alone by `per_read`, recording each in `reads`. `Err` names a loopback
/// carrier or Skarbiec client that could not be had.
async fn read_bearers(
    tunnel: Arc<Tunnel>,
    remote_host: &str,
    remote_port: u16,
    token_file: &str,
    namespaces: &BTreeMap<String, ObjectApiNamespace>,
    per_read: Duration,
    reads: &mut Vec<Value>,
) -> Result<Read, String> {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, u16::default()))
        .await
        .map_err(|error| format!("no loopback port to carry the reads could be bound: {error}"))?;
    let local = listener
        .local_addr()
        .map_err(|error| format!("the bound loopback port could not be read back: {error}"))?;
    let channel_host = remote_host.to_string();
    let carrier = tokio::spawn(async move {
        while let Ok((mut client, _)) = listener.accept().await {
            let tunnel = Arc::clone(&tunnel);
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
    .map_err(|error| format!("no Skarbiec client as consumer stado: {error}"));
    let client = match client {
        Ok(client) => client,
        Err(cause) => {
            carrier.abort();
            return Err(cause);
        }
    };
    let mut outcome = Read::Answered;
    for (namespace, policy) in namespaces {
        let named = format!("{namespace} ({})", policy.item());
        let read_started = Instant::now();
        let answer =
            tokio::time::timeout(per_read, client.read_declared_revision(policy.item(), BEARER_FIELD))
                .await;
        let seconds = read_started.elapsed().as_secs_f64();
        let Ok(answer) = answer else {
            outcome = Read::Unanswered(named);
            break;
        };
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
        if !answered && matches!(outcome, Read::Answered) {
            outcome = Read::Refused(format!("{named}: {detail}"));
        }
    }
    carrier.abort();
    Ok(outcome)
}
