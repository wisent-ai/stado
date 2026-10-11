//! Removing the instance a cancellation fence captured: only through the
//! dedicated provider lease that still owns it, and acknowledged apart from
//! the later provider read.

use crate::machine::MachineError;
use crate::models::{job_state, Job};
use crate::queue::JobStorage;

use super::super::jobs::RecordedInstance;
use super::{failure, read_marker, AGENT_INSTANCE_PREFIX};

/// A delete acknowledgement remains separate from the later provider read.
pub(crate) async fn request_provider_removal(
    store: &JobStorage,
    job: &Job,
) -> Result<Option<RecordedInstance>, MachineError> {
    let job_id = job.job_id.as_str();
    let marker = read_marker(store, job_id).await?.ok_or_else(|| {
        failure(format!(
            "cancellation fence for {job_id} disappeared before provider removal"
        ))
    })?;
    let Some(allocation) = marker.allocation else {
        return Ok(None);
    };
    let instance = RecordedInstance {
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
