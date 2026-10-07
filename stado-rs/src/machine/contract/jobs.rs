//! The lifecycle prefixes, the machine-facing job view, and the provider
//! instance a job is recorded as holding.

use serde_json::{Map, Value};

use crate::models::Job;
use crate::queue::leases::{LeaseError, ProviderLeaseStore};
use crate::queue::runs;
use crate::queue::JobStorage;

/// Prefixes probed by [`crate::machine::MachineFacade::lookup_job`]. Terminal
/// and running destinations precede queue so a crash-retained source fence
/// cannot mask the newer lifecycle state. The order is this facade's; the
/// names are the queue's.
pub const JOB_PREFIXES: [&str; 6] = [
    runs::CANCELLED,
    runs::FAILED,
    runs::UPLOADED,
    runs::COMPLETED,
    runs::RUNNING,
    runs::QUEUE,
];

/// Machine-facing job view (Python `normalize_job`): the queue/ prefix reads
/// as "queued", Option fields become null.
pub fn normalize_job(job: &Job) -> Value {
    let state = if job.state == "queue" {
        "queued"
    } else {
        job.state.as_str()
    };
    let mut out = Map::new();
    out.insert("job_id".into(), Value::from(job.job_id.as_str()));
    out.insert("run_id".into(), Value::from(job.run_id.as_str()));
    out.insert("batch_id".into(), Value::from(job.batch_id.as_str()));
    out.insert("state".into(), Value::from(state));
    // Whether this job has stopped moving, decided here from the queue's own
    // terminal set. Every reader used to decide it again by matching state
    // words — Oko's routines did it in Swift — and a reader that misses one
    // waits forever for a job that already finished.
    out.insert(
        "terminal".into(),
        Value::from(runs::TERMINAL_PREFIXES.contains(&state)),
    );
    out.insert("command".into(), Value::from(job.command.as_str()));
    out.insert("provider".into(), Value::from(job.provider.as_str()));
    out.insert("gpu_type".into(), Value::from(job.gpu_type.as_str()));
    out.insert("gpu_mem_gb".into(), Value::from(job.gpu_mem_gb));
    out.insert(
        "machine_type".into(),
        Value::from(job.machine_type.as_str()),
    );
    // These are recorded assignment/request fields, not measured hardware.
    out.insert("region".into(), Value::from(job.region.as_str()));
    out.insert("preemptible".into(), Value::from(job.preemptible));
    out.insert("assigned_to".into(), Value::from(job.assigned_to.as_str()));
    out.insert("instance_ref".into(), serde_json::json!(job.instance_ref));
    out.insert(
        "worker_allocation".into(),
        serde_json::json!(job.worker_allocation),
    );
    let allocation_kind = job
        .instance_ref
        .as_deref()
        .filter(|reference| !reference.is_empty())
        .map(|reference| {
            if reference.starts_with(AGENT_INSTANCE_PREFIX) {
                "agent"
            } else {
                "provider"
            }
        });
    out.insert("allocation_kind".into(), serde_json::json!(allocation_kind));
    out.insert("restarts".into(), Value::from(job.restarts));
    out.insert(
        "submitter_restarts".into(),
        Value::from(job.submitter_restarts),
    );
    out.insert("last_restart".into(), serde_json::json!(job.last_restart));
    out.insert("created_at".into(), Value::from(job.created_at.as_str()));
    out.insert(
        "started_at".into(),
        job.started_at
            .as_deref()
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    out.insert(
        "completed_at".into(),
        job.completed_at
            .as_deref()
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    out.insert(
        "failed_at".into(),
        job.failed_at
            .as_deref()
            .map(Value::from)
            .unwrap_or(Value::Null),
    );
    out.insert(
        "error".into(),
        job.error.as_deref().map(Value::from).unwrap_or(Value::Null),
    );
    out.insert("output_uri".into(), Value::from(job.output_uri.as_str()));
    Value::Object(out)
}

/// A provider instance a job is recorded as holding, plus the blob the
/// record came from so an operator can go look at it.
#[derive(Debug, Clone)]
pub struct RecordedInstance {
    /// Provider name for [`crate::providers::get_provider`].
    pub provider: String,
    /// Provider-native reference, `"name@zone"` on GCE.
    pub instance_ref: String,
    /// Observed job field or provider lease that supplied the reference.
    pub source: String,
    /// An agent transport reference, on either a local or a cloud worker.
    /// It does not grant this job ownership of the worker VM.
    pub agent: bool,
}

/// The agent transport prefix, not a declaration of physical provider.
pub const AGENT_INSTANCE_PREFIX: &str = "local@";

/// Resolve the supplied job snapshot, then its dedicated provider lease.
///
/// Agent VMs are shared and have no per-job VM lease. Box allocations use
/// provider leases; an old job reference alone does not authorize deletion.
/// The caller retains this snapshot's execution identity with the reference.
pub async fn recorded_instance(
    store: &JobStorage,
    job: &Job,
) -> Result<Option<RecordedInstance>, LeaseError> {
    fn found(provider: &str, instance_ref: &str, source: String) -> RecordedInstance {
        RecordedInstance {
            provider: provider.to_string(),
            instance_ref: instance_ref.to_string(),
            source,
            agent: instance_ref.starts_with(AGENT_INSTANCE_PREFIX),
        }
    }
    let job_id = &job.job_id;
    if let Some(instance_ref) = job
        .instance_ref
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        return Ok(Some(found(
            &job.provider,
            instance_ref,
            format!("job:{job_id}#instance_ref"),
        )));
    }
    if let Some(worker) = job
        .worker_allocation
        .as_ref()
        .filter(|_| job.started_at.is_some())
    {
        if !worker.host.is_empty() {
            return Ok(Some(RecordedInstance {
                provider: job.provider.clone(),
                instance_ref: format!("{AGENT_INSTANCE_PREFIX}{}", worker.host),
                source: format!("job:{job_id}#worker_allocation.host"),
                agent: true,
            }));
        }
    }
    let stored = match ProviderLeaseStore::new(store.clone()).load(job_id).await {
        Ok(stored) => stored,
        // The lease store refuses any job id it cannot encode as a safe
        // path, which also means it can never have written one for this
        // job. Absence, not a failure to look.
        Err(LeaseError::Value(_)) => None,
        Err(exc) => return Err(exc),
    };
    let Some(lease) = stored else {
        return Ok(None);
    };
    if lease.provider_resource_id.is_empty() {
        return Ok(None);
    }
    let source = format!("provider-leases/{job_id}.json");
    Ok(Some(found(
        &lease.provider,
        &lease.provider_resource_id,
        source,
    )))
}
