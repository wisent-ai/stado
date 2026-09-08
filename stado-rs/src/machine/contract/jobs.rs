//! The lifecycle prefixes, the machine-facing job view, and the provider
//! instance a job is recorded as holding.

use serde_json::{Map, Value};

use crate::models::Job;
use crate::queue::leases::{LeaseError, ProviderLeaseStore};
use crate::queue::JobStorage;

/// Prefixes probed by [`crate::machine::MachineFacade::lookup_job`]. Terminal
/// and running destinations precede queue so a crash-retained source fence
/// cannot mask the newer lifecycle state.
pub const JOB_PREFIXES: [&str; 6] = [
    "cancelled",
    "failed",
    "uploaded",
    "completed",
    "running",
    "queue",
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
    out.insert("command".into(), Value::from(job.command.as_str()));
    out.insert("provider".into(), Value::from(job.provider.as_str()));
    out.insert("gpu_type".into(), Value::from(job.gpu_type.as_str()));
    out.insert("gpu_mem_gb".into(), Value::from(job.gpu_mem_gb));
    out.insert(
        "machine_type".into(),
        Value::from(job.machine_type.as_str()),
    );
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
    /// Blob path the reference was read from.
    pub source: String,
    /// True for the `local@<host>` pseudo-refs a local agent writes. There
    /// is no cloud instance behind those and no provider call to make.
    pub local: bool,
}

/// The `instance_ref` prefix a local agent stamps on a job it claims. Not a
/// cloud resource: `queue::submit` never routes it to a provider and both
/// cancel paths skip the delete for it.
pub const LOCAL_INSTANCE_PREFIX: &str = "local@";

/// Resolve the cloud instance `job_id` is recorded as holding.
///
/// NO Python original. Two independent records exist and only one of them
/// was ever consulted:
///
///  1. the job document's `provider` / `instance_ref` fields, written by
///     the dispatcher once the instance is up, and
///  2. `provider-leases/<job_id>.json`
///     (`queue::leases::ProviderLeaseStore::load`), which records
///     `provider_resource_id` from the moment the allocation is *attempted*.
///
/// The lease is written first and cleared last, so it covers the two
/// windows the job document does not: a dispatch that created the instance
/// but died before stamping the job, and a job whose document was already
/// rewritten (moved to `failed/` by a partial cancel) while the instance
/// stayed up. Both leak a running VM that nothing else reclaims — the
/// billing gap `stado cancel --terminate` exists to close.
///
/// The job document wins when both carry a reference: it is what the
/// dispatcher confirmed, whereas a lease can still name a resource whose
/// creation call ultimately failed.
pub async fn recorded_instance(
    store: &JobStorage,
    job_id: &str,
) -> Result<Option<RecordedInstance>, LeaseError> {
    fn found(provider: &str, instance_ref: &str, source: String) -> RecordedInstance {
        RecordedInstance {
            provider: provider.to_string(),
            instance_ref: instance_ref.to_string(),
            source,
            local: instance_ref.starts_with(LOCAL_INSTANCE_PREFIX),
        }
    }
    for prefix in JOB_PREFIXES {
        let Some(job) = store.read_job(prefix, job_id).await? else {
            continue;
        };
        let instance_ref = job.instance_ref.as_deref().unwrap_or_default();
        if !instance_ref.is_empty() {
            let source = format!("{prefix}/{job_id}.json");
            return Ok(Some(found(&job.provider, instance_ref, source)));
        }
        break;
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
