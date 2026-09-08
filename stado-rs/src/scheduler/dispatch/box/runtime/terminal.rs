//! Terminal Box lease transitions: complete, fail and release.
//!
//! Port of `stado/scheduler/dispatch/box/runtime.py`.

use crate::models::{job_state, Job};
use crate::queue::leases::{LeaseState, ProviderLease};

use super::super::output::upload_artifacts;
use super::super::BoxDispatchError;
use super::{now_iso, BoxRuntime};

impl BoxRuntime<'_> {
    /// Python `complete`.
    pub(crate) async fn complete(
        &self,
        job: &mut Job,
        lease: &mut ProviderLease,
        success: bool,
        error: &str,
    ) -> Result<(), BoxDispatchError> {
        lease.result_state = if success {
            job_state::COMPLETED
        } else {
            job_state::FAILED
        }
        .to_string();
        lease.last_error = error.chars().take(512).collect();
        if lease.state == LeaseState::Running.as_str() {
            let (owner, token) = (lease.owner_id.clone(), lease.fence_token.clone());
            lease.transition(LeaseState::Collecting, &owner, &token)?;
            self.save(lease).await?;
        }
        self.resume_terminal(job, lease).await
    }

    /// Python `fail`.
    pub(crate) async fn fail(
        &self,
        job: &mut Job,
        lease: &mut ProviderLease,
        error: &str,
        resource_released: bool,
    ) -> Result<(), BoxDispatchError> {
        lease.result_state = job_state::FAILED.to_string();
        lease.last_error = error.chars().take(512).collect();
        let terminal = [
            LeaseState::Failed.as_str(),
            LeaseState::Releasing.as_str(),
            LeaseState::Released.as_str(),
        ];
        if !terminal.contains(&lease.state.as_str()) {
            let (owner, token) = (lease.owner_id.clone(), lease.fence_token.clone());
            lease.transition(LeaseState::Failed, &owner, &token)?;
            self.save(lease).await?;
        }
        if resource_released && lease.state != LeaseState::Released.as_str() {
            if lease.state == LeaseState::Failed.as_str() {
                let (owner, token) = (lease.owner_id.clone(), lease.fence_token.clone());
                lease.transition(LeaseState::Releasing, &owner, &token)?;
                self.save(lease).await?;
            }
            let (owner, token) = (lease.owner_id.clone(), lease.fence_token.clone());
            lease.transition(LeaseState::Released, &owner, &token)?;
            self.save(lease).await?;
        }
        self.resume_terminal(job, lease).await
    }

    /// Python `resume_terminal`: drive COLLECTING/FAILED/RELEASING leases
    /// to RELEASED and move the job to its terminal prefix.
    pub(crate) async fn resume_terminal(
        &self,
        job: &mut Job,
        lease: &mut ProviderLease,
    ) -> Result<(), BoxDispatchError> {
        if lease.state == LeaseState::Collecting.as_str() {
            if lease.result_state == job_state::COMPLETED {
                let box_id = lease.provider_resource_id.clone();
                let mut keepalive = self.keepalive_handle(lease);
                upload_artifacts(
                    self.store,
                    &self.provider.client,
                    job,
                    &box_id,
                    &mut keepalive,
                )
                .await?;
            }
            let (owner, token) = (lease.owner_id.clone(), lease.fence_token.clone());
            lease.transition(LeaseState::Releasing, &owner, &token)?;
            self.save(lease).await?;
        }
        if lease.state == LeaseState::Failed.as_str() {
            let (owner, token) = (lease.owner_id.clone(), lease.fence_token.clone());
            lease.transition(LeaseState::Releasing, &owner, &token)?;
            self.save(lease).await?;
        }
        if lease.state == LeaseState::Releasing.as_str() {
            self.provider
                .release_box(&lease.provider_resource_id)
                .await?;
            let (owner, token) = (lease.owner_id.clone(), lease.fence_token.clone());
            lease.transition(LeaseState::Released, &owner, &token)?;
            self.save(lease).await?;
        }
        if lease.state != LeaseState::Released.as_str() {
            return Ok(());
        }
        let now = now_iso();
        if lease.result_state == job_state::COMPLETED {
            job.state = job_state::COMPLETED.to_string();
            job.completed_at = Some(now);
            self.store.move_job(job, "running", "completed").await?;
        } else {
            job.state = job_state::FAILED.to_string();
            job.error = Some(if lease.last_error.is_empty() {
                "Box workload failed".to_string()
            } else {
                lease.last_error.clone()
            });
            job.failed_at = Some(now);
            self.store.move_job(job, "running", "failed").await?;
        }
        Ok(())
    }
}
