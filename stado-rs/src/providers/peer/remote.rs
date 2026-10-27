//! The peer's store, opened with the bearer it issued to us, and what the
//! job records on either side say about the work peers ran.
//!
//! The bearer is read through this host's Skarbiec channel
//! ([`crate::providers::host_credential`]) by the role the declaration names,
//! and kept in an owner-only file under this host's market directory: the
//! object backend reads its bearer from a file, and the lending worker is a
//! separate process that has to read the same one. Nothing else writes it.
//!
//! Every count is read from job records: the peer's `completed/` for what
//! this host lent, ours for what peer hosts ran here. A record missing its
//! start or completion is named as unmeasured, never given a value.

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::providers::host_credential::read_role_field;
use crate::queue::{JobStorage, Listing, StadoObjectBackend};

use super::{PeerDeclaration, PeerError, LENDING_KIND, TOKEN_FIELD};

/// The lifecycle prefix completed jobs are kept under.
const COMPLETED: &str = "completed";

/// This host's directory for one peer: its bearer file and lending homes.
pub fn peer_directory(peer: &str) -> PathBuf {
    crate::config_file::expand_tilde("~/.stado/market/stado").join(peer)
}

/// The bearer file the backend and the lending worker read.
pub fn bearer_file(peer: &str) -> PathBuf {
    peer_directory(peer).join("token")
}

/// Read the peer's bearer from Skarbiec and keep it in its owner-only file.
pub async fn install_bearer(peer: &PeerDeclaration) -> Result<PathBuf, PeerError> {
    let reading = read_role_field(&peer.credential_role, TOKEN_FIELD).await;
    let Some(token) = reading.key.clone() else {
        return Err(PeerError::Credential(format!(
            "no bearer for peer {} on this host: {}. `stado market readiness --provider stado \
             --peer {}` reports whether the vault declares the item playing {} and which grant \
             is missing",
            peer.name,
            reading.cause(),
            peer.name,
            peer.credential_role
        )));
    };
    let path = bearer_file(&peer.name);
    // Owner-only, whole and flushed with its directory, so the queue client
    // never reads half a bearer and the stage no longer has a fixed name.
    crate::primitives::document::persist(&path, token.as_bytes()).map_err(|error| {
        PeerError::Local(format!("cannot install the bearer file {}: {error}", path.display()))
    })?;
    Ok(path)
}

/// The peer's queue, opened with its bearer.
pub async fn open(peer: &PeerDeclaration) -> Result<JobStorage, PeerError> {
    let token_file = install_bearer(peer).await?;
    let ca_file = match &peer.ca_file {
        Some(path) => path.as_str(),
        None => "",
    };
    let backend = StadoObjectBackend::new(
        &peer.origin,
        &peer.namespace,
        &token_file.to_string_lossy(),
        ca_file,
    )?;
    Ok(JobStorage::with_backend_and_bucket(
        Arc::new(backend),
        "stado",
        peer.namespace.clone(),
    ))
}

/// Whether the peer's queue holds work, and how many of its jobs run.
/// The queue is read one name deep: whether it is empty is all a lending
/// decision needs, and the answer says so.
pub async fn queue_state(store: &JobStorage) -> Result<Value, PeerError> {
    let queued = store.list_page("queue/", "", Some(NonZeroUsize::MIN)).await?;
    let running = store.list_paths("running/", Listing::Every).await?.len();
    Ok(json!({"queue_empty": queued.is_empty(), "running": running}))
}

/// Seconds between two RFC 3339 times, refused when they measure no work.
fn seconds_between(started: &str, completed: &str) -> Result<i64, String> {
    let parse = |text: &str| {
        chrono::DateTime::parse_from_rfc3339(text)
            .map_err(|error| format!("{text:?} is not an RFC 3339 time: {error}"))
    };
    let span = parse(completed)? - parse(started)?;
    match span.num_seconds() {
        seconds if seconds.is_positive() => Ok(seconds),
        seconds => Err(format!(
            "it completed {seconds} seconds after it started, which measures no work"
        )),
    }
}

/// Every completed job a lending worker ran in `store`, with its host and
/// measured seconds, and the ones that cannot be measured with why. With
/// `host`, only that host's.
pub async fn lent_work(store: &JobStorage, host: Option<&str>) -> Result<Value, PeerError> {
    let mut jobs = Vec::new();
    let mut unmeasured = Vec::new();
    for job in store.list_jobs(COMPLETED).await? {
        let Some(allocation) = job.worker_allocation.as_ref() else {
            continue;
        };
        if allocation.kind != LENDING_KIND || host.is_some_and(|host| allocation.host != host) {
            continue;
        }
        let measured = match (job.started_at.as_deref(), job.completed_at.as_deref()) {
            (Some(started), Some(completed)) => seconds_between(started, completed),
            (None, _) => Err("its record states no start".to_string()),
            (_, None) => Err("its record states no completion".to_string()),
        };
        match measured {
            Ok(seconds) => jobs.push(json!({
                "job": job.job_id,
                "host": allocation.host,
                "started_at": job.started_at,
                "completed_at": job.completed_at,
                "seconds": seconds,
            })),
            Err(reason) => unmeasured.push(json!({"job": job.job_id, "host": allocation.host, "reason": reason})),
        }
    }
    let seconds: i64 = jobs
        .iter()
        .filter_map(|job| job.get("seconds").and_then(Value::as_i64))
        .sum();
    Ok(json!({"jobs": jobs, "seconds": seconds, "unmeasured": unmeasured}))
}

/// The amount `seconds` come to at `price_gpu` US dollars per GPU-hour.
pub fn amount(seconds: i64, price_gpu: f64) -> f64 {
    let hour = time::Duration::HOUR.whole_seconds();
    seconds as f64 / hour as f64 * price_gpu
}
