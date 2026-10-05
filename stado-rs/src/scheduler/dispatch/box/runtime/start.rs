//! Box workload start: READY -> STARTING -> RUNNING.
//!
//! Port of `stado/scheduler/dispatch/box/runtime.py`.

use crate::models::Job;
use crate::providers::r#box::BoxError;
use crate::queue::leases::{LeaseState, ProviderLease};

use super::super::output::{command_wrapper, recover_prompt_id, runtime_paths, shell_quote};
use super::super::BoxDispatchError;
use super::{now_iso, BoxRuntime};

impl BoxRuntime<'_> {
    /// Python `start`: READY -> STARTING -> RUNNING (idempotent).
    pub(crate) async fn start(
        &self,
        job: &mut Job,
        lease: &mut ProviderLease,
    ) -> Result<bool, BoxDispatchError> {
        let allow_prompt_submit = match lease.state.as_str() {
            state if state == LeaseState::Ready.as_str() => {
                if lease.operation_id.is_empty() {
                    lease.operation_id = format!("stado-{}", job.job_id);
                }
                lease.operation_started_at = now_iso();
                let (owner, token) = (lease.owner_id.clone(), lease.fence_token.clone());
                lease.transition(LeaseState::Starting, &owner, &token)?;
                self.save(lease).await?;
                true
            }
            state if state == LeaseState::Starting.as_str() => false,
            other => {
                return Err(BoxDispatchError::value(format!(
                    "cannot start Box workload from {other}"
                )));
            }
        };
        if job.executor == "box-prompt" {
            return self.start_prompt(job, lease).await;
        }
        self.start_command(job, lease, allow_prompt_submit).await?;
        Ok(true)
    }

    /// Python `_start_command`: write run.sh (once), then the idempotent
    /// launch shell (exit-file/launch-marker/pid guarded).
    async fn start_command(
        &self,
        job: &mut Job,
        lease: &mut ProviderLease,
        mut fresh: bool,
    ) -> Result<(), BoxDispatchError> {
        let box_id = lease.provider_resource_id.clone();
        let paths = runtime_paths(&job.job_id);
        if !fresh {
            match self
                .provider
                .client
                .read_file(&box_id, &paths.launch, "utf-8")
                .await
            {
                Ok(_) => {}
                Err(BoxError::Api(api)) if api.status == 404 => fresh = true,
                Err(err) => return Err(err.into()),
            }
        }
        if fresh {
            self.provider
                .client
                .write_file(
                    &box_id,
                    &paths.script,
                    &command_wrapper(job, &paths),
                    "utf-8",
                )
                .await?;
        }
        let root = shell_quote(&paths.root);
        let script = shell_quote(&paths.script);
        let pid = shell_quote(&paths.pid);
        let exit_path = shell_quote(&paths.exit);
        let marker = shell_quote(&paths.launch);
        let operation = shell_quote(&lease.operation_id);
        let launch = format!(
            "mkdir -p {root} && chmod 700 {script} && \
             if test -s {exit_path}; then true; \
             elif test -s {marker}; then \
             test -s {pid} && kill -0 $(cat {pid}) 2>/dev/null; \
             else ((printf '%s' {operation} >{marker}.tmp && \
             mv {marker}.tmp {marker}) || exit 70; \
             setsid nohup {script} >/dev/null 2>&1 & p=$!; \
             printf '%s' \"$p\" >{pid}.tmp; mv {pid}.tmp {pid}; \
             kill -0 \"$p\" 2>/dev/null || test -s {exit_path}); fi"
        );
        let result = self
            .provider
            .client
            .execute_command(&box_id, &launch, "")
            .await?;
        if !result.success {
            self.fail(
                job,
                lease,
                "Box launch marker exists without a live or completed process",
                false,
            )
            .await?;
            return Ok(());
        }
        let (owner, token) = (lease.owner_id.clone(), lease.fence_token.clone());
        lease.transition(LeaseState::Running, &owner, &token)?;
        self.save(lease).await
    }

    /// Python `_prompt_marker`.
    fn prompt_marker(lease: &ProviderLease) -> String {
        format!("[stado-operation:{}]", lease.operation_id)
    }

    /// Python `_start_prompt`. A prompt is submitted only when the box's
    /// whole prompt log carries no prompt marked with this operation: a
    /// submission the box accepted is in its log, so an earlier attempt that
    /// never reached the box is submitted now instead of waiting on a clock.
    async fn start_prompt(
        &self,
        job: &mut Job,
        lease: &mut ProviderLease,
    ) -> Result<bool, BoxDispatchError> {
        if job.prompt.is_empty() || job.prompt_provider.is_empty() {
            return Err(BoxDispatchError::value(
                "box-prompt requires prompt and prompt_provider",
            ));
        }
        let box_id = lease.provider_resource_id.clone();
        let marker = Self::prompt_marker(lease);
        let mut prompt_id = lease.prompt_id.clone();
        if prompt_id.is_empty() {
            let mut keepalive = self.keepalive_handle(lease);
            prompt_id =
                recover_prompt_id(&self.provider.client, &box_id, &marker, &mut keepalive).await?;
        }
        if prompt_id.is_empty() {
            let run = self
                .provider
                .client
                .prompt(
                    &box_id,
                    &format!("{marker}\n{}", job.prompt),
                    &job.prompt_provider,
                    &job.prompt_model,
                    &job.prompt_reasoning_effort,
                )
                .await?;
            prompt_id = run.prompt_id;
        }
        if prompt_id.is_empty() {
            self.fail(
                job,
                lease,
                "Box accepted the prompt without returning its id",
                false,
            )
            .await?;
            return Ok(true);
        }
        lease.prompt_id = prompt_id;
        let (owner, token) = (lease.owner_id.clone(), lease.fence_token.clone());
        lease.transition(LeaseState::Running, &owner, &token)?;
        self.save(lease).await?;
        Ok(true)
    }
}
