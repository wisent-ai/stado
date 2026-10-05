//! Cancellation fences retain the allocation before a terminal transition clears it.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::encoding::{canonical_json, utcnow};
use super::jobs::{recorded_instance, AGENT_INSTANCE_PREFIX};
use crate::machine::MachineError;
use crate::models::{job_state, Job, WorkerAllocation};
use crate::queue::JobStorage;

#[derive(Deserialize, Serialize)]
struct Allocation {
    provider: String,
    instance_ref: String,
    started_at: Option<String>,
    restarts: i64,
    source: String,
    captured_at: String,
    worker_allocation: Option<WorkerAllocation>,
}

fn failure(message: impl Into<String>) -> MachineError {
    MachineError::retryable("CANCEL_FAILED", message)
}

pub(crate) async fn fence_cancellation(
    store: &JobStorage,
    job_id: &str,
) -> Result<Job, MachineError> {
    let facade = crate::machine::MachineFacade::with_store(store.clone(), crate::config::bucket());
    facade.lookup_job(job_id).await?;
    let path = format!("cancellations/{job_id}.json");
    store
        .create_text_if_absent(
            &path,
            &canonical_json(&json!({
                "job_id": job_id, "requested_at": utcnow(),
            })),
        )
        .await?;
    // A claim can have won after the first read but before the fence.
    let job = facade.lookup_job(job_id).await?;
    capture_cancellation_allocation(store, &job).await?;
    Ok(job)
}

/// Called only after the durable fence exists and before releasing its resource.
/// A CAS conflict retains the fence and refuses deletion; the caller can reconcile.
pub(crate) async fn capture_cancellation_allocation(
    store: &JobStorage,
    job: &Job,
) -> Result<(), MachineError> {
    let Some(instance) = recorded_instance(store, job).await.map_err(|error| {
        failure(format!(
            "read cancellation allocation for {}: {error}",
            job.job_id
        ))
    })?
    else {
        return Ok(());
    };
    let path = format!("cancellations/{}.json", job.job_id);
    let record = store.read_text_versioned(&path).await?.ok_or_else(|| {
        failure(format!(
            "cancellation fence {path} disappeared before allocation capture"
        ))
    })?;
    let mut marker: Value = serde_json::from_str(&record.content)
        .map_err(|error| failure(format!("read cancellation fence {path}: {error}")))?;
    if marker.get("job_id").and_then(Value::as_str) != Some(job.job_id.as_str()) {
        return Err(failure(format!(
            "cancellation fence {path} names a different job"
        )));
    }
    if let Some(value) = marker.get("allocation").filter(|value| !value.is_null()) {
        let previous = Allocation::deserialize(value)
            .map_err(|error| failure(format!("read allocation in {path}: {error}")))?;
        if previous.provider != instance.provider
            || previous.instance_ref != instance.instance_ref
            || (previous.started_at.is_some() && previous.started_at != job.started_at)
            || (previous.worker_allocation.is_some()
                && previous.worker_allocation != job.worker_allocation)
        {
            return Err(failure(format!(
                "cancellation allocation in {path} differs from the observed execution"
            )));
        }
        if previous.started_at == job.started_at
            && previous.restarts == job.restarts
            && previous.worker_allocation == job.worker_allocation
        {
            return Ok(());
        }
        // A first claim or a refused restart can advance metadata without a new execution.
    }
    let allocation = Allocation {
        provider: instance.provider,
        instance_ref: instance.instance_ref,
        started_at: job.started_at.clone(),
        restarts: job.restarts,
        source: instance.source,
        captured_at: utcnow(),
        worker_allocation: job.worker_allocation.clone(),
    };
    marker["allocation"] = serde_json::to_value(allocation)
        .map_err(|error| failure(format!("encode cancellation allocation in {path}: {error}")))?;
    store
        .compare_and_swap_text(&path, &record.version, &canonical_json(&marker))
        .await
        .map_err(|error| {
            failure(format!(
                "capture cancellation allocation in {path}: {error}"
            ))
        })?;
    Ok(())
}

pub(in crate::machine) async fn provider_cleanup(
    store: &JobStorage,
    job: &Job,
) -> Result<Option<Value>, MachineError> {
    if !job_state::is_terminal(&job.state) {
        return Ok(None);
    }
    let Some(marker) = read_marker(store, &job.job_id).await? else {
        return Ok(None);
    };
    let Some(allocation) = marker.allocation else {
        return Ok(None);
    };
    let observation = async {
        let worker = allocation.worker_allocation.as_ref().ok_or_else(|| {
            crate::providers::ProviderError::Value(
                "captured allocation has no worker-origin observation".into(),
            )
        })?;
        if allocation.instance_ref.strip_prefix(AGENT_INSTANCE_PREFIX) != Some(worker.host.as_str())
        {
            return Err(crate::providers::ProviderError::Value(
                "captured worker origin differs from the execution's agent reference".into(),
            ));
        }
        if let Some(error) = &worker.error {
            return Err(crate::providers::ProviderError::Value(format!(
                "worker identity observation: {error}"
            )));
        }
        let resource = worker.resource.as_ref().ok_or_else(|| {
            crate::providers::ProviderError::Value(
                "captured worker has no physical resource identity".into(),
            )
        })?;
        if matches!(resource, crate::models::WorkerResource::Local) {
            return Err(crate::providers::ProviderError::NotImplemented(
                "local policy does not identify a provider VM; workload cleanup needs its own receipt".into(),
            ));
        }
        crate::providers::get_provider(resource.provider().as_str())?
            .instance_removed(resource)
            .await
    }
    .await;
    let (removed, state, evidence, error) = match observation {
        Ok(observation) => (
            Some(observation.removed),
            observation.state,
            Some(observation.evidence),
            None,
        ),
        Err(error) => (None, None, None, Some(error.to_string())),
    };
    Ok(Some(json!({
        "job_id": job.job_id,
        "operation": "observe_instance_removal",
        "allocation": allocation,
        "observed_at": utcnow(),
        "removed": removed,
        "state": state,
        "error": error,
        "evidence": evidence,
    })))
}

#[derive(Deserialize)]
struct Marker {
    job_id: String,
    allocation: Option<Allocation>,
}

async fn read_marker(store: &JobStorage, job_id: &str) -> Result<Option<Marker>, MachineError> {
    let path = format!("cancellations/{job_id}.json");
    let Some(record) = store.read_text_versioned(&path).await? else {
        return Ok(None);
    };
    let marker: Marker = serde_json::from_str(&record.content)
        .map_err(|error| failure(format!("read cancellation fence {path}: {error}")))?;
    if marker.job_id != job_id {
        return Err(failure(format!(
            "cancellation fence {path} names a different job"
        )));
    }
    if let Some(allocation) = &marker.allocation {
        // An agent reference (`local@HOST`) is the transport a job ran over,
        // not a provider resource the job owns, and a job pinned to a fleet
        // host names no provider at all; only a provider allocation has to
        // say whose it is. Refusing an agent reference without one made
        // `stado cancel` of every pinned build job answer infra_down.
        let agent = allocation.instance_ref.starts_with(AGENT_INSTANCE_PREFIX);
        if (allocation.provider.is_empty() && !agent)
            || allocation.instance_ref.is_empty()
            || allocation.restarts < 0
        {
            return Err(failure(format!(
                "cancellation allocation in {path} has invalid ownership fields: provider \
                 {:?}, instance {:?}, restarts {}",
                allocation.provider, allocation.instance_ref, allocation.restarts
            )));
        }
    }
    Ok(Some(marker))
}

/// A delete acknowledgement remains separate from the later provider read.
pub(crate) async fn request_provider_removal(
    store: &JobStorage,
    job: &Job,
) -> Result<Option<super::jobs::RecordedInstance>, MachineError> {
    let job_id = job.job_id.as_str();
    let marker = read_marker(store, job_id).await?.ok_or_else(|| {
        failure(format!(
            "cancellation fence for {job_id} disappeared before provider removal"
        ))
    })?;
    let Some(allocation) = marker.allocation else {
        return Ok(None);
    };
    let instance = super::jobs::RecordedInstance {
        agent: allocation.instance_ref.starts_with(AGENT_INSTANCE_PREFIX),
        provider: allocation.provider,
        instance_ref: allocation.instance_ref,
        source: format!(
            "cancellations/{job_id}.json#allocation (captured from {})",
            allocation.source
        ),
    };
    if !instance.agent {
        if job_state::is_terminal(&job.state)
            && !crate::capabilities::ProviderId::Box.matches(&instance.provider)
        {
            return Ok(None);
        }
        let lease = crate::queue::leases::ProviderLeaseStore::new(store.clone())
            .load(job_id)
            .await
            .map_err(|error| failure(format!("read deletion ownership for {job_id}: {error}")))?
            .filter(|lease| lease.state != crate::queue::leases::LeaseState::Released.as_str());
        let Some(lease) = lease else {
            if job_state::is_terminal(&job.state) {
                return Ok(None);
            }
            return Err(failure(format!(
                "refusing to delete {}: no active dedicated provider lease for {job_id}",
                instance.instance_ref
            )));
        };
        if !crate::capabilities::ProviderId::Box.matches(&lease.provider)
            || lease.job_id != job_id
            || lease.provider != instance.provider
            || lease.provider_resource_id != instance.instance_ref
            || lease
                .state
                .parse::<crate::queue::leases::LeaseState>()
                .is_err()
        {
            return Err(failure(format!(
                "refusing to delete {}: no current dedicated Box lease matching cancellation ownership",
                instance.instance_ref
            )));
        }
        let provider =
            crate::providers::get_provider(&instance.provider).map_err(|error| match error {
                error @ crate::providers::ProviderError::Disabled(_) => {
                    MachineError::new("PROVIDER_DISABLED", error.to_string())
                }
                error @ crate::providers::ProviderError::NotEnabled(_) => {
                    MachineError::new("PROVIDER_NOT_ENABLED", error.to_string())
                }
                error => failure(error.to_string()),
            })?;
        provider
            .delete_instance(&instance.instance_ref)
            .await
            .map_err(|error| {
                failure(format!(
                    "failed to delete instance {} recorded in {}: {error}",
                    instance.instance_ref, instance.source
                ))
            })?;
    }
    Ok(Some(instance))
}
