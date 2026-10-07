//! `stado repair stado --step agent-slots --target HOST`: free the slots a
//! host's agent holds for jobs that are no longer running anywhere.
//!
//! An agent keeps a slot until it has moved its job out of `running/`. When
//! another writer settled the job first (a cancel, the reaper, or a newer
//! Stado finishing a transition this agent could not read), an agent built
//! before the slot release in `slots::lifecycle::advance` retries that move
//! forever, so the slot never frees: a held release build keeps its Cargo
//! directory claimed, and every later build of that product on the host is
//! declined. The slot lives only in the agent's memory, so the one way to
//! free it on such an agent is the agent's own service restart.
//!
//! Every live slot holds a job in `running/` that names the host (pinned to
//! it, or allocated to it as its worker). The step reads the slot count the
//! agent itself publishes and the live running jobs the store attributes to
//! the host, and restarts the agent only when the agent counts more slots
//! than there are such jobs: those slots hold nothing that can still finish.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::models::Job;
use crate::queue::JobStorage;

/// The service an agent runs as; its restart is the declared one.
const AGENT_SERVICE: &str = "stado";

/// Whether `word` (a pin or a worker host) names `host`: its registry name,
/// or the `<kind>-<hostname>` consumer identity its agent claims under.
fn names_host(
    word: &str,
    host: &crate::targets::ComputeTarget,
    registry: &crate::targets::Registry,
) -> bool {
    !word.is_empty()
        && (word == host.name
            || word
                .strip_prefix(format!("{}-", host.kind).as_str())
                .is_some_and(|identity| {
                    registry
                        .lookup_self(identity)
                        .ok()
                        .flatten()
                        .is_some_and(|found| found.name == host.name)
                }))
}

/// Whether a running job is held on `host`: pinned to it, or allocated to it
/// as its worker.
fn attributed(
    job: &Job,
    host: &crate::targets::ComputeTarget,
    registry: &crate::targets::Registry,
) -> bool {
    names_host(&job.pinned_host, host, registry)
        || job
            .worker_allocation
            .as_ref()
            .is_some_and(|allocation| names_host(&allocation.host, host, registry))
}

pub(super) async fn apply(target: &str) -> Result<Value, CmdError> {
    let registry = crate::targets::load_registry_auto()
        .await
        .map_err(CmdError::from)?;
    let host = crate::cli::canonical_host(target).await?;
    let store = JobStorage::new().await?;
    let mut live = Vec::new();
    for job_id in store.list_job_ids("running").await? {
        let Some(job) = store.read_job("running", &job_id).await? else {
            continue;
        };
        if !attributed(&job, &host, &registry) {
            continue;
        }
        let mut settled = false;
        for prefix in crate::queue::runs::TERMINAL_PREFIXES {
            settled |= store.read_job(prefix, &job_id).await?.is_some();
        }
        if !settled {
            live.push(job_id);
        }
    }
    let mut slots = None;
    for (consumer, row) in crate::queue::capacity::read_publications(&store).await? {
        if names_host(&consumer, &host, &registry) {
            slots = row.payload.get("running_jobs").and_then(Value::as_i64);
        }
    }
    let slots = slots.ok_or_else(|| {
        CmdError::click(format!(
            "{}: its agent publishes no running job count, so its slots cannot be compared \
             with the jobs it holds",
            host.name
        ))
        .stating(crate::primitives::failure::FailureCode::NotFound)
    })?;
    let live_count = i64::try_from(live.len()).map_err(|error| {
        CmdError::click(format!(
            "{}: live running jobs cannot be counted: {error}",
            host.name
        ))
    })?;
    if slots <= live_count {
        return Ok(json!({
            "target": host.name, "published_slots": slots, "live_jobs": live, "restarted": false
        }));
    }
    let declared = crate::cli::service::declared_matching(AGENT_SERVICE, Some(&host.name)).await?;
    let service = declared.first().ok_or_else(|| {
        CmdError::click(format!(
            "{}: its agent publishes {slots} slots for {live_count} live running jobs, but no \
             `{AGENT_SERVICE}` service is declared on it, so there is no declared unit to restart",
            host.name
        ))
        .stating(crate::primitives::failure::FailureCode::NotFound)
    })?;
    let runner = crate::deploy::production_runner();
    let report = crate::deploy::service::restart_service(&host, service, &runner)
        .await
        .map_err(CmdError::from)?;
    if !report.succeeded("restarted") {
        return Err(CmdError::click(format!(
            "{}: its agent publishes {slots} slots for {live_count} live running jobs, and \
             restarting {} failed: {}",
            host.name,
            service.unit_id(),
            report.failure()
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    Ok(json!({
        "target": host.name, "published_slots": slots, "live_jobs": live,
        "restarted": true, "unit": service.unit_id()
    }))
}
