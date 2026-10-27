//! Live-worker projection for the makespan matcher: which workers are
//! admitting jobs right now, how much VRAM each already owes to running
//! work, and the per-job runtime estimate both paths share. Split out of
//! `makespan/mod.rs`; the matcher itself lives in `matcher`.
//!
//! Every field the projection reads is one the publisher or the job record
//! always writes, so a document without it is refused with its path rather
//! than read as zero.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use chrono::{DateTime, Utc};
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

use crate::queue::{JobStorage, Listing, StorageError};

use super::history::{extract_model_task, History};

static INSTANCE_HOST_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[^@]+@(?P<host>.+)$").expect("static regex compiles"));

/// A runtime estimate as a job states it: a duration in seconds that is not
/// empty. The job record writes an unset estimate as zero, which states none;
/// one that is no duration (negative, not a number) is said and states none.
fn stated(seconds: f64) -> Option<f64> {
    match Duration::try_from_secs_f64(seconds) {
        Ok(duration) if duration.is_zero() => None,
        Ok(_) => Some(seconds),
        Err(error) => {
            tracing::warn!("a job's runtime_seconds_estimate {seconds} is no duration: {error}");
            None
        }
    }
}

/// The stated estimate, else the history of the job's (model, task).
fn estimate(command: &str, stated: Option<f64>, history: &History) -> Option<f64> {
    if stated.is_some() {
        return stated;
    }
    history.get(&extract_model_task(command)?).copied()
}

/// Per-job runtime estimate in seconds. Returns None when neither an
/// explicit estimate nor a matching history entry is available; the
/// caller leaves the job unassigned with a log naming the missing
/// (model, task). A new (model, task) combo must either set
/// runtime_seconds_estimate at submit time or wait for a sibling job
/// to complete and seed history; the matcher refuses to guess.
/// Python `_estimate_runtime` (factored onto primitives so both the
/// queued-job path and the running-blob seed path share it).
pub(super) fn estimate_runtime(
    command: &str,
    runtime_seconds_estimate: f64,
    history: &History,
) -> Option<f64> {
    estimate(command, stated(runtime_seconds_estimate), history)
}

/// Live-worker projection state.
#[derive(Debug, Default)]
pub struct AgentInfo {
    pub kind: String,
    pub available_accelerators: BTreeMap<String, i64>,
    pub total_vram_gb: i64,
    /// (finish_offset_seconds, vram_gb) per projected active job.
    pub active_jobs: Vec<(f64, i64)>,
}

/// A whole number of gigabytes a document states as an integer or a float.
fn gigabytes(doc: &Value, field: &str) -> Option<i64> {
    doc.get(field).and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_f64().map(|float| float as i64))
    })
}

/// Workers with a fresh publication and an explicit positive admission
/// decision. A live row alone is not capacity: a worker can keep publishing
/// while disk, RAM, CPU, or accelerator state prevents another claim.
pub(super) async fn live_agents(
    store: &JobStorage,
    now: DateTime<Utc>,
) -> Result<BTreeMap<String, AgentInfo>, StorageError> {
    let paths = store.list_paths("capacity/", Listing::Every).await?;
    let texts = download_many(store, &paths).await?;
    let mut agents: BTreeMap<String, AgentInfo> = BTreeMap::new();
    for text in texts.into_iter().flatten() {
        let doc: Value = serde_json::from_str(&text)?;
        let refused =
            |what: &str| StorageError::Other(format!("makespan: capacity blob {what}: {doc}"));
        // Every agent is keyed by its consumer id; a row without one was keyed
        // "", so every such row folded into one phantom agent.
        let cid = doc
            .get("consumer_id")
            .and_then(Value::as_str)
            .ok_or_else(|| refused("names no consumer_id"))?
            .to_string();
        let pub_at = doc
            .get("published_at")
            .and_then(Value::as_str)
            .ok_or_else(|| refused("states no published_at"))?;
        let published = DateTime::parse_from_rfc3339(pub_at).map_err(|e| {
            StorageError::Other(format!(
                "makespan: capacity blob has malformed published_at {pub_at:?}: {e}"
            ))
        })?;
        let stamp = Some(published.with_timezone(&Utc));
        if !crate::queue::capacity::publication_live(&doc, stamp, now) {
            continue;
        }
        if doc.get("accepting_jobs").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        let mut available_accelerators: BTreeMap<String, i64> = BTreeMap::new();
        if let Some(published) = doc.get("available_accelerators").and_then(Value::as_object) {
            for (accelerator, count) in published {
                // A count that is not an integer was read as no free cards.
                let count = count.as_i64().ok_or_else(|| {
                    StorageError::Other(format!(
                        "makespan: capacity blob of {cid} states {accelerator} free as {count}, not a count"
                    ))
                })?;
                available_accelerators.insert(accelerator.clone(), count);
            }
        }
        let kind = doc
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| refused("states no kind"))?
            .to_string();
        let total_vram_gb =
            gigabytes(&doc, "total_vram_gb").ok_or_else(|| refused("states no total_vram_gb"))?;
        agents.insert(
            cid,
            AgentInfo {
                kind,
                available_accelerators,
                total_vram_gb,
                active_jobs: Vec::new(),
            },
        );
    }
    Ok(agents)
}

/// For each running/ blob, locate the executing agent (by hostname
/// in instance_ref) and add an active_slot covering its remaining
/// runtime. Without this, a freshly-claimed long job looks invisible
/// and the matcher would pile more work onto an already-loaded agent.
/// Python `_seed_running_jobs`.
pub(super) async fn seed_running_jobs(
    store: &JobStorage,
    agents: &mut BTreeMap<String, AgentInfo>,
    now: DateTime<Utc>,
    history: &History,
    log_fn: &dyn Fn(&str),
) -> Result<(), StorageError> {
    // consumer_id is "<kind>-<hostname>" (queue/capacity.publish_capacity).
    let mut host_to_cid: HashMap<String, String> = HashMap::new();
    for cid in agents.keys() {
        if let Some((_, host)) = cid.split_once('-') {
            host_to_cid.insert(host.to_string(), cid.clone());
        }
    }
    let paths = store.list_paths("running/", Listing::Every).await?;
    let texts = download_many(store, &paths).await?;
    for (path, text) in paths.iter().zip(&texts) {
        let Some(text) = text else { continue }; // moved to completed/failed mid-tick
        let doc: Value = serde_json::from_str(text)?;
        let refused =
            |what: &str| StorageError::Other(format!("makespan: running blob {path} {what}"));
        let iref = doc
            .get("instance_ref")
            .and_then(Value::as_str)
            .ok_or_else(|| refused("names no instance_ref"))?;
        let Some(host) = INSTANCE_HOST_RE
            .captures(iref)
            .and_then(|caps| caps.name("host"))
            .map(|host| host.as_str())
        else {
            return Err(refused(&format!(
                "has malformed instance_ref {iref:?}; cannot map to consumer_id"
            )));
        };
        let Some(cid) = host_to_cid.get(host) else {
            // Running job points to an agent we no longer track as live.
            // Leave its VRAM out of the projection; the reaper will move
            // the running blob to failed on its own pass. Logging only.
            log_fn(&format!(
                "makespan: running {path} on dead host {host}; skipping projection"
            ));
            continue;
        };
        let Some(st) = doc.get("started_at").and_then(Value::as_str) else {
            return Err(refused("has no started_at"));
        };
        let started = DateTime::parse_from_rfc3339(st).map_err(|e| {
            StorageError::Other(format!(
                "makespan: running blob {path} bad started_at {st:?}: {e}"
            ))
        })?;
        let command = doc
            .get("command")
            .and_then(Value::as_str)
            .ok_or_else(|| refused("has no command"))?;
        let runtime_hint = doc
            .get("runtime_seconds_estimate")
            .and_then(Value::as_f64)
            .and_then(stated);
        let Some(est) = estimate(command, runtime_hint, history) else {
            // Admin/maintenance commands have no parseable (model, task); the
            // matcher can't predict their finish time. Agent-side smi_free
            // enforces actual VRAM at claim time.
            log_fn(&format!("makespan: skip running {path} for seeding"));
            continue;
        };
        let estimated = Duration::try_from_secs_f64(est).map_err(|error| {
            refused(&format!(
                "has a runtime estimate {est} that is no duration: {error}"
            ))
        })?;
        // A start stamped after this clock's now has run for no time yet.
        let elapsed = match (now - started.with_timezone(&Utc)).to_std() {
            Ok(elapsed) => elapsed,
            Err(_ahead) => Duration::ZERO,
        };
        // A job past its estimate owes no more time.
        let remaining = estimated.saturating_sub(elapsed).as_secs_f64();
        let vram = gigabytes(&doc, "gpu_mem_gb").ok_or_else(|| refused("states no gpu_mem_gb"))?;
        if let Some(info) = agents.get_mut(cid) {
            info.active_jobs.push((remaining, vram));
        }
    }
    Ok(())
}

/// Parallel-download the given blob paths, as wide as the machine is, path
/// order preserved. A missing blob (TOCTOU between list and download) is
/// None; any other error propagates.
pub(crate) async fn download_many(
    store: &JobStorage,
    paths: &[String],
) -> Result<Vec<Option<String>>, StorageError> {
    use futures::StreamExt;
    futures::stream::iter(paths)
        .map(|path| store.download_text(path))
        .buffered(crate::queue::migrations::bulk_workers()?)
        .collect::<Vec<Result<Option<String>, StorageError>>>()
        .await
        .into_iter()
        .collect()
}
