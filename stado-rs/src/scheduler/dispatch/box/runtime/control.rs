//! Interrupting and cancelling an in-flight Box workload.
//!
//! Port of `stado/scheduler/dispatch/box/runtime.py`.

use crate::models::Job;
use crate::providers::r#box::BoxError;
use crate::queue::leases::ProviderLease;

use super::super::output::{runtime_paths, shell_quote};
use super::super::BoxDispatchError;
use super::{BoxRuntime, CONTROL_TIMEOUT_SECONDS};

impl BoxRuntime<'_> {
    /// Python `interrupt`: cancel the in-flight command or prompt,
    /// tolerating already-gone/already-stopped boxes.
    pub(crate) async fn interrupt(
        &self,
        job: &Job,
        lease: &ProviderLease,
    ) -> Result<(), BoxDispatchError> {
        let tolerated = |err: &BoxError| match err {
            BoxError::Api(api) => {
                api.status == 404
                    || matches!(api.code.as_str(), "no_active_work" | "machine_not_running")
            }
            _ => false,
        };
        let result = if job.executor == "box-prompt" {
            self.provider
                .client
                .interrupt(&lease.provider_resource_id)
                .await
                .map(|_| ())
        } else {
            let pid_path = runtime_paths(&job.job_id).pid;
            self.provider
                .client
                .execute_command(
                    &lease.provider_resource_id,
                    &format!("kill -- -$(cat {})", shell_quote(&pid_path)),
                    "",
                    CONTROL_TIMEOUT_SECONDS,
                )
                .await
                .map(|_| ())
        };
        match result {
            Ok(()) => Ok(()),
            Err(err) if tolerated(&err) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    /// Python `cancel`.
    pub(crate) async fn cancel(
        &self,
        job: &mut Job,
        lease: &mut ProviderLease,
    ) -> Result<(), BoxDispatchError> {
        self.interrupt(job, lease).await?;
        self.fail(job, lease, "cancelled", false).await
    }
}
