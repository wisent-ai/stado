//! `stado repair stado --step agent-slots --target HOST`: free the slots a
//! host's agent holds for jobs another writer already settled.
//!
//! An agent keeps a slot until it has moved its job out of `running/`. When
//! another writer settled the job first (a cancel, the reaper, or a newer
//! Stado finishing a transition this agent could not read), an agent built
//! before the slot release in `slots::lifecycle::advance` retries that move
//! forever, so the slot never frees: a held release build keeps its Cargo
//! directory claimed, and every later build of that product on the host is
//! declined. The slot lives only in the agent's memory, so the one way to
//! free it on such an agent is the agent's own service restart, and this
//! step restarts it only when the store shows a job pinned to the host that
//! is still in `running/` and already sits in a terminal prefix.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::models::Job;
use crate::queue::JobStorage;

/// The service an agent runs as; its restart is the declared one.
const AGENT_SERVICE: &str = "stado";

pub(super) async fn apply(target: &str) -> Result<Value, CmdError> {
    let registry = crate::targets::load_registry_auto()
        .await
        .map_err(CmdError::from)?;
    let host = crate::cli::canonical_host(target).await?;
    let store = JobStorage::new().await?;
    let identity_prefix = format!("{}-", host.kind);
    let pinned_here = |job: &Job| -> bool {
        !job.pinned_host.is_empty()
            && (job.pinned_host == host.name
                || job
                    .pinned_host
                    .strip_prefix(identity_prefix.as_str())
                    .is_some_and(|identity| {
                        registry
                            .lookup_self(identity)
                            .ok()
                            .flatten()
                            .is_some_and(|found| found.name == host.name)
                    }))
    };
    let mut held = Vec::new();
    for job_id in store.list_job_ids("running").await? {
        let Some(job) = store.read_job("running", &job_id).await? else {
            continue;
        };
        if !pinned_here(&job) {
            continue;
        }
        for prefix in crate::queue::runs::TERMINAL_PREFIXES {
            if let Some(settled) = store.read_job(prefix, &job_id).await? {
                held.push(json!({ "job_id": job_id, "settled_as": settled.state }));
                break;
            }
        }
    }
    if held.is_empty() {
        return Ok(json!({ "target": host.name, "held": held, "restarted": false }));
    }
    let declared = crate::cli::service::declared_matching(AGENT_SERVICE, Some(&host.name)).await?;
    let service = declared.first().ok_or_else(|| {
        CmdError::click(format!(
            "{}: its agent holds slots for settled jobs, but no `{AGENT_SERVICE}` service is \
             declared on it, so there is no declared unit to restart",
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
            "{}: its agent holds slots for settled jobs ({}), and restarting {} failed: {}",
            host.name,
            held.iter()
                .filter_map(|entry| entry["job_id"].as_str())
                .collect::<Vec<_>>()
                .join(", "),
            service.unit_id(),
            report.failure()
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    Ok(json!({ "target": host.name, "held": held, "restarted": true, "unit": service.unit_id() }))
}
