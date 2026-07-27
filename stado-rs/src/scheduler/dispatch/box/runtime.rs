//! Restartable structured execution inside an allocated Box.
//!
//! Port of `stado/scheduler/dispatch/box/runtime.py`.

use chrono::{DateTime, Utc};

use crate::models::{job_state, Job};
use crate::providers::r#box::{BoxError, BoxProvider};
use crate::queue::leases::{LeaseState, ProviderLease, ProviderLeaseStore};
use crate::queue::JobStorage;

use super::output::{
    command_wrapper, file_content, prompt_output, recover_prompt_id, runtime_paths,
    shell_quote, upload_artifacts,
};
use super::BoxDispatchError;

const CONTROL_TIMEOUT_SECONDS: i64 = 60;
const PROMPT_RECOVERY_SECONDS: i64 = 120;
/// Python `_keepalive`'s `int("300")` owner-TTL renewal.
const KEEPALIVE_OWNER_TTL_SECONDS: i64 = 300;

/// Python `datetime.now(timezone.utc).isoformat()`.
pub(crate) fn now_iso() -> String {
    crate::models::isoformat_utc(Utc::now())
}

/// Python `datetime.fromisoformat(value.replace("Z", "+00:00"))`, lenient
/// (None when the stored timestamp is unparseable).
pub(crate) fn parse_iso(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value.replace('Z', "+00:00"))
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Lease owner-TTL renewal handed to the output helpers so long
/// paginations/uploads renew between network calls, exactly like Python's
/// `keepalive` callable parameter.
pub(crate) struct Keepalive<'r, 'l> {
    runtime: &'r BoxRuntime<'r>,
    lease: &'l mut ProviderLease,
}

impl Keepalive<'_, '_> {
    pub(crate) async fn ping(&mut self) -> Result<(), BoxDispatchError> {
        self.runtime.keepalive(self.lease).await
    }
}

/// Python `BoxRuntime`.
pub(crate) struct BoxRuntime<'a> {
    store: &'a JobStorage,
    provider: &'a BoxProvider,
    leases: &'a ProviderLeaseStore,
}

impl<'a> BoxRuntime<'a> {
    pub(crate) fn new(
        store: &'a JobStorage,
        provider: &'a BoxProvider,
        leases: &'a ProviderLeaseStore,
    ) -> Self {
        BoxRuntime { store, provider, leases }
    }

    async fn save(&self, lease: &mut ProviderLease) -> Result<(), BoxDispatchError> {
        let version = lease.version.clone();
        *lease = self.leases.save(lease.clone(), &version).await?;
        Ok(())
    }

    /// Python `_keepalive`.
    async fn keepalive(&self, lease: &mut ProviderLease) -> Result<(), BoxDispatchError> {
        let (owner, token) = (lease.owner_id.clone(), lease.fence_token.clone());
        lease.renew_owner(&owner, &token, KEEPALIVE_OWNER_TTL_SECONDS)?;
        self.save(lease).await
    }

    /// A Keepalive handle borrowing this runtime and the lease.
    fn keepalive_handle<'r, 'l>(&'r self, lease: &'l mut ProviderLease) -> Keepalive<'r, 'l> {
        Keepalive { runtime: self, lease }
    }

    /// Python `start`: READY -> STARTING -> RUNNING (idempotent).
    /// Returns false when a prompt's start outcome is still unknown and the
    /// recovery deadline has not lapsed.
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
            return self.start_prompt(job, lease, allow_prompt_submit).await;
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
            match self.provider.client.read_file(&box_id, &paths.launch, "utf-8").await {
                Ok(_) => {}
                Err(BoxError::Api(api)) if api.status == 404 => fresh = true,
                Err(err) => return Err(err.into()),
            }
        }
        if fresh {
            self.provider
                .client
                .write_file(&box_id, &paths.script, &command_wrapper(job, &paths), "utf-8")
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
             printf '%s' \"$p\" >{pid}.tmp; mv {pid}.tmp {pid}; sleep 1; \
             kill -0 \"$p\" 2>/dev/null || test -s {exit_path}); fi"
        );
        let result = self
            .provider
            .client
            .execute_command(&box_id, &launch, "", CONTROL_TIMEOUT_SECONDS)
            .await?;
        if !result.success {
            self.fail(job, lease, "Box launch marker exists without a live or completed process", false)
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

    /// Python `_start_prompt`.
    async fn start_prompt(
        &self,
        job: &mut Job,
        lease: &mut ProviderLease,
        allow_submit: bool,
    ) -> Result<bool, BoxDispatchError> {
        if job.prompt.is_empty() || job.prompt_provider.is_empty() {
            return Err(BoxDispatchError::value("box-prompt requires prompt and prompt_provider"));
        }
        let box_id = lease.provider_resource_id.clone();
        let marker = Self::prompt_marker(lease);
        let mut prompt_id = lease.prompt_id.clone();
        if prompt_id.is_empty() {
            let mut keepalive = self.keepalive_handle(lease);
            prompt_id = recover_prompt_id(&self.provider.client, &box_id, &marker, &mut keepalive).await?;
        }
        if prompt_id.is_empty() && allow_submit {
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
            let started = parse_iso(&lease.operation_started_at)
                .ok_or_else(|| BoxDispatchError::value("invalid operation_started_at"))?;
            let age = (Utc::now() - started).num_seconds();
            if age < PROMPT_RECOVERY_SECONDS {
                return Ok(false);
            }
            self.fail(job, lease, "Box prompt start outcome remained unknown", false).await?;
            return Ok(true);
        }
        lease.prompt_id = prompt_id;
        let (owner, token) = (lease.owner_id.clone(), lease.fence_token.clone());
        lease.transition(LeaseState::Running, &owner, &token)?;
        self.save(lease).await?;
        Ok(true)
    }

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
        let exit_text = match self.provider.client.read_file(&box_id, &paths.exit, "utf-8").await {
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
            let path = if key == "stdout" { &paths.stdout } else { &paths.stderr };
            let content = match self.provider.client.read_file(&box_id, path, "utf-8").await {
                Ok(value) => file_content(&value)?,
                Err(BoxError::Api(api)) if api.status == 404 => String::new(),
                Err(err) => return Err(err.into()),
            };
            self.keepalive(lease).await?;
            self.store
                .upload_text(&format!("status/{}/output/command_{key}.log", job.job_id), &content)
                .await?;
            self.keepalive(lease).await?;
        }
        let success = exit_code == 0;
        self.complete(
            job,
            lease,
            success,
            if success { "" } else { "Box command or verification failed" },
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
            return Err(BoxDispatchError::runtime("Box prompt lease omitted prompt id"));
        }
        let box_id = lease.provider_resource_id.clone();
        let run = self.provider.client.prompt_status(&box_id, &lease.prompt_id).await?;
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
            .upload_text(&format!("status/{}/output/prompt_output.txt", job.job_id), &output)
            .await?;
        self.keepalive(lease).await?;
        let success = run.status == "finished";
        let error = format!("Box prompt {}", run.status);
        self.complete(job, lease, success, if success { "" } else { &error }).await?;
        Ok(true)
    }

    /// Python `complete`.
    pub(crate) async fn complete(
        &self,
        job: &mut Job,
        lease: &mut ProviderLease,
        success: bool,
        error: &str,
    ) -> Result<(), BoxDispatchError> {
        lease.result_state =
            if success { job_state::COMPLETED } else { job_state::FAILED }.to_string();
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
                upload_artifacts(self.store, &self.provider.client, job, &box_id, &mut keepalive)
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
            self.provider.release_box(&lease.provider_resource_id).await?;
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
            self.provider.client.interrupt(&lease.provider_resource_id).await.map(|_| ())
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
