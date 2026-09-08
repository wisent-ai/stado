//! Box workload reconciliation while the lease is RUNNING.
//!
//! Port of `stado/scheduler/dispatch/box/runtime.py`.

use crate::models::Job;
use crate::providers::r#box::BoxError;
use crate::queue::leases::ProviderLease;

use super::super::output::{file_content, prompt_output, runtime_paths};
use super::super::BoxDispatchError;
use super::BoxRuntime;

impl BoxRuntime<'_> {
    /// Python `reconcile_running`. Returns false while the workload is
    /// still in flight.
    pub(crate) async fn reconcile_running(
        &self,
        job: &mut Job,
        lease: &mut ProviderLease,
    ) -> Result<bool, BoxDispatchError> {
        self.keepalive(lease).await?;
        if job.executor == "box-prompt" {
            return self.reconcile_prompt(job, lease).await;
        }
        self.reconcile_command(job, lease).await
    }

    /// Python `_reconcile_command`: exit-file polling, bounded log upload.
    async fn reconcile_command(
        &self,
        job: &mut Job,
        lease: &mut ProviderLease,
    ) -> Result<bool, BoxDispatchError> {
        let box_id = lease.provider_resource_id.clone();
        let paths = runtime_paths(&job.job_id);
        let exit_text = match self
            .provider
            .client
            .read_file(&box_id, &paths.exit, "utf-8")
            .await
        {
            Ok(value) => {
                let text = file_content(&value)?;
                self.keepalive(lease).await?;
                text
            }
            Err(BoxError::Api(api)) if api.status == 404 => {
                self.keepalive(lease).await?;
                return Ok(false);
            }
            Err(err) => return Err(err.into()),
        };
        let exit_code: i64 = exit_text
            .trim()
            .parse()
            .map_err(|_| BoxError::transport("Box command exit file is invalid"))?;
        for key in ["stdout", "stderr"] {
            let path = if key == "stdout" {
                &paths.stdout
            } else {
                &paths.stderr
            };
            let content = match self.provider.client.read_file(&box_id, path, "utf-8").await {
                Ok(value) => file_content(&value)?,
                Err(BoxError::Api(api)) if api.status == 404 => String::new(),
                Err(err) => return Err(err.into()),
            };
            self.keepalive(lease).await?;
            self.store
                .upload_text(
                    &format!("status/{}/output/command_{key}.log", job.job_id),
                    &content,
                )
                .await?;
            self.keepalive(lease).await?;
        }
        let success = exit_code == 0;
        self.complete(
            job,
            lease,
            success,
            if success {
                ""
            } else {
                "Box command or verification failed"
            },
        )
        .await?;
        Ok(true)
    }

    /// Python `_reconcile_prompt`: prompt-status polling, bounded output
    /// upload.
    async fn reconcile_prompt(
        &self,
        job: &mut Job,
        lease: &mut ProviderLease,
    ) -> Result<bool, BoxDispatchError> {
        if lease.prompt_id.is_empty() {
            return Err(BoxDispatchError::runtime(
                "Box prompt lease omitted prompt id",
            ));
        }
        let box_id = lease.provider_resource_id.clone();
        let run = self
            .provider
            .client
            .prompt_status(&box_id, &lease.prompt_id)
            .await?;
        self.keepalive(lease).await?;
        if !run.done {
            return Ok(false);
        }
        let output = {
            let prompt_id = lease.prompt_id.clone();
            let mut keepalive = self.keepalive_handle(lease);
            prompt_output(&self.provider.client, &box_id, &prompt_id, &mut keepalive).await?
        };
        self.store
            .upload_text(
                &format!("status/{}/output/prompt_output.txt", job.job_id),
                &output,
            )
            .await?;
        self.keepalive(lease).await?;
        let success = run.status == "finished";
        let error = format!("Box prompt {}", run.status);
        self.complete(job, lease, success, if success { "" } else { &error })
            .await?;
        Ok(true)
    }
}
