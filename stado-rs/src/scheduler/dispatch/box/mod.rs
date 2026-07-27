//! Fenced dispatch and exhaustive reconciliation for Box-backed jobs.
//!
//! Port of `stado/scheduler/dispatch/box/__init__.py`.
//!
//! (Python also defines an unused `_TERMINAL_BOX_STATES` frozenset — the
//! terminal checks are written inline at each use site, as ported here.)

pub mod output;
pub mod runtime;

use chrono::Utc;
use uuid::Uuid;

use crate::models::{job_state, Job};
use crate::providers::r#box::{BoxError, BoxProvider};
use crate::queue::leases::{LeaseError, LeaseState, ProviderLease, ProviderLeaseStore};
use crate::queue::{JobStorage, StorageError};

use runtime::{now_iso, parse_iso, BoxRuntime};

const OWNER_TTL_SECONDS: i64 = 300;
const QUEUE_SCAN_CAP: usize = 25;
const START_RECOVERY_SECONDS: i64 = 120;

/// Python `_READY_BOX_STATES`.
const READY_BOX_STATES: [&str; 3] = ["ready", "idle", "running"];
/// Python `_RENEWED_STATES`.
fn renewed_state(state: &str) -> bool {
    matches!(
        state,
        s if s == LeaseState::Provisioning.as_str()
            || s == LeaseState::Ready.as_str()
            || s == LeaseState::Starting.as_str()
            || s == LeaseState::Running.as_str()
    )
}

/// Box-dispatch layer error. Python raises `ValueError` for invalid lease
/// transitions / workload shapes, `RuntimeError` for state-machine
/// violations, and lets Box/lease/storage errors propagate.
#[derive(Debug, thiserror::Error)]
pub enum BoxDispatchError {
    /// Box API / transport / validation failures.
    #[error(transparent)]
    Box(#[from] BoxError),
    /// Fenced lease failures (conflicts, illegal transitions).
    #[error(transparent)]
    Lease(#[from] LeaseError),
    /// Queue storage failures.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// Python `ValueError`.
    #[error("{0}")]
    Value(String),
    /// Python `RuntimeError`.
    #[error("{0}")]
    Runtime(String),
}

impl BoxDispatchError {
    pub(crate) fn value(message: impl Into<String>) -> Self {
        BoxDispatchError::Value(message.into())
    }

    pub(crate) fn runtime(message: impl Into<String>) -> Self {
        BoxDispatchError::Runtime(message.into())
    }

    /// Python `except LeaseConflict`.
    fn is_conflict(&self) -> bool {
        matches!(self, BoxDispatchError::Lease(err) if err.is_conflict())
    }

    /// Python `type(exc).__name__` for the failure log line.
    fn type_name(&self) -> &'static str {
        match self {
            BoxDispatchError::Box(_) => "BoxError",
            BoxDispatchError::Lease(_) => "LeaseError",
            BoxDispatchError::Storage(_) => "StorageError",
            BoxDispatchError::Value(_) => "ValueError",
            BoxDispatchError::Runtime(_) => "RuntimeError",
        }
    }
}

/// Python `_log_failure`.
fn log_failure(job_id: &str, exc: &BoxDispatchError) {
    let text: String = exc.to_string().replace(['\r', '\n'], " ").chars().take(512).collect();
    eprintln!("[box] job={job_id} {}: {text}", exc.type_name());
}

/// Python `_fail_queued`.
async fn fail_queued(
    store: &JobStorage,
    job: &mut Job,
    message: &str,
) -> Result<(), BoxDispatchError> {
    job.state = job_state::FAILED.to_string();
    job.error = Some(message.chars().take(512).collect());
    job.failed_at = Some(now_iso());
    store.move_job(job, "queue", "failed").await?;
    Ok(())
}

/// Python `_relinquish`: best-effort owner release; every failure is
/// swallowed.
async fn relinquish(leases: &ProviderLeaseStore, lease: Option<ProviderLease>) {
    let Some(mut lease) = lease else { return };
    // A corrupt stored timestamp can no longer be renewed meaningfully;
    // treat it as expired (Python would raise out of owner_expired, but
    // the finally-block swallow is the operational intent).
    if lease.owner_expired().unwrap_or(true) {
        return;
    }
    let (owner, token) = (lease.owner_id.clone(), lease.fence_token.clone());
    let result: Result<(), BoxDispatchError> = async {
        lease.relinquish(&owner, &token)?;
        let version = lease.version.clone();
        leases.save(lease, &version).await?;
        Ok(())
    }
    .await;
    let _ = result;
}

/// Python `dispatch_box_jobs`: admit pinned queued jobs and allocate
/// available Box capacity.
pub async fn dispatch_box_jobs(
    store: &JobStorage,
    provider: &BoxProvider,
    owner_id: &str,
) -> Result<i64, BoxDispatchError> {
    let leases = ProviderLeaseStore::new(store.clone());
    let mut scheduled: i64 = 0;
    for mut job in store.list_jobs_priority_first("queue", QUEUE_SCAN_CAP).await? {
        if !matches!(job.provider.as_str(), "box" | "box-ascii") {
            continue;
        }
        if !job.pin_to_provider {
            fail_queued(store, &mut job, "Box jobs must set pin_to_provider=true").await?;
            continue;
        }
        let decision = provider.admit(&job);
        if !decision.accepted {
            fail_queued(store, &mut job, &decision.reasons.join("; ")).await?;
            continue;
        }
        let resource_ttl =
            if job.box_ttl_seconds != 0 { job.box_ttl_seconds } else { provider.ttl_seconds };
        let mut lease: Option<ProviderLease> = None;
        let mut resource_recorded = false;
        let outcome: Result<bool, BoxDispatchError> = async {
            let mut acquired = leases
                .acquire(&job.job_id, "box", owner_id, OWNER_TTL_SECONDS, resource_ttl)
                .await?;
            let scheduled = provision_and_move(
                store,
                provider,
                &leases,
                &mut job,
                &mut acquired,
                resource_ttl,
                &mut resource_recorded,
            )
            .await?;
            lease = Some(acquired);
            Ok(scheduled)
        }
        .await;
        match outcome {
            Ok(did_schedule) => {
                relinquish(&leases, lease).await;
                if did_schedule {
                    scheduled += 1;
                }
            }
            Err(err) if err.is_conflict() => {
                relinquish(&leases, lease).await;
            }
            Err(err) => {
                log_failure(&job.job_id, &err);
                // Python's except-handler failures propagate out of the
                // loop, but the finally-block relinquish runs first.
                let handler: Result<(), BoxDispatchError> = async {
                    if !resource_recorded {
                        if let Some(l) = lease.as_mut() {
                            if l.state == LeaseState::Allocating.as_str() {
                                l.last_error =
                                    "Box allocation outcome is unknown; resource TTL remains the bound"
                                        .to_string();
                                l.result_state = job_state::FAILED.to_string();
                                let (owner, token) =
                                    (l.owner_id.clone(), l.fence_token.clone());
                                l.transition(LeaseState::Failed, &owner, &token)?;
                                let version = l.version.clone();
                                *l = leases.save(l.clone(), &version).await?;
                            }
                        }
                        fail_queued(store, &mut job, &err.to_string()).await?;
                    }
                    Ok(())
                }
                .await;
                relinquish(&leases, lease).await;
                handler?;
            }
        }
    }
    Ok(scheduled)
}

/// The resource-provisioning half of Python's `dispatch_box_jobs` loop
/// body (everything inside the try after `acquire`).
#[allow(clippy::too_many_arguments)]
async fn provision_and_move(
    store: &JobStorage,
    provider: &BoxProvider,
    leases: &ProviderLeaseStore,
    job: &mut Job,
    lease: &mut ProviderLease,
    resource_ttl: i64,
    resource_recorded: &mut bool,
) -> Result<bool, BoxDispatchError> {
    if !lease.provider_resource_id.is_empty() {
        *resource_recorded = true;
        let terminal = [
            LeaseState::Failed.as_str(),
            LeaseState::Releasing.as_str(),
            LeaseState::Released.as_str(),
        ];
        if terminal.contains(&lease.state.as_str()) {
            fail_queued(store, job, &format!("Box lease is already {}", lease.state)).await?;
            return Ok(false);
        }
        job.instance_ref = Some(lease.provider_resource_id.clone());
    } else {
        let created = provider.create_box(Some(resource_ttl)).await?;
        lease.provider_resource_id = created.box_id.clone();
        let (owner, token) = (lease.owner_id.clone(), lease.fence_token.clone());
        lease.transition(LeaseState::Provisioning, &owner, &token)?;
        let version = lease.version.clone();
        *lease = leases.save(lease.clone(), &version).await?;
        *resource_recorded = true;
        job.instance_ref = Some(created.box_id);
    }
    job.state = job_state::RUNNING.to_string();
    if job.started_at.as_deref().is_none_or(str::is_empty) {
        job.started_at = Some(now_iso());
    }
    store.move_job(job, "queue", "running").await?;
    Ok(true)
}

/// Python `_box_state`: "gone" maps a 404.
async fn box_state(provider: &BoxProvider, lease: &ProviderLease) -> Result<String, BoxError> {
    match provider.client.get_box(&lease.provider_resource_id).await {
        Ok(info) => Ok(info.state),
        Err(BoxError::Api(api)) if api.status == 404 => Ok("gone".to_string()),
        Err(err) => Err(err),
    }
}

/// Python `_reconcile_one`. Returns true when the lease state advanced
/// (or the lease reached a terminal disposition) this tick.
async fn reconcile_one(
    provider: &BoxProvider,
    runtime: &BoxRuntime<'_>,
    leases: &ProviderLeaseStore,
    job: &mut Job,
    lease: &mut ProviderLease,
) -> Result<bool, BoxDispatchError> {
    let state = lease.state.parse::<LeaseState>()?;
    if matches!(
        state,
        LeaseState::Collecting | LeaseState::Failed | LeaseState::Releasing | LeaseState::Released
    ) {
        runtime.resume_terminal(job, lease).await?;
        return Ok(true);
    }
    if lease.provider_resource_id.is_empty() {
        runtime.fail(job, lease, "Box lease has no provider resource", true).await?;
        return Ok(true);
    }
    let box_state = box_state(provider, lease).await?;
    if box_state == "gone" || box_state == "archived" {
        runtime
            .fail(job, lease, &format!("Box became {box_state} before completion"), true)
            .await?;
        return Ok(true);
    }
    if box_state == "error" {
        runtime.fail(job, lease, "Box entered error state", false).await?;
        return Ok(true);
    }
    let ttl = if job.box_ttl_seconds != 0 { job.box_ttl_seconds } else { provider.ttl_seconds };
    if renewed_state(&lease.state) {
        provider.renew_box(&lease.provider_resource_id, Some(ttl)).await?;
        let (owner, token) = (lease.owner_id.clone(), lease.fence_token.clone());
        lease.renew_resource(&owner, &token, ttl)?;
        lease.renew_owner(&owner, &token, OWNER_TTL_SECONDS)?;
        let version = lease.version.clone();
        *lease = leases.save(lease.clone(), &version).await?;
    }
    if lease.state == LeaseState::Provisioning.as_str() {
        if !READY_BOX_STATES.contains(&box_state.as_str()) {
            return Ok(false);
        }
        let (owner, token) = (lease.owner_id.clone(), lease.fence_token.clone());
        lease.transition(LeaseState::Ready, &owner, &token)?;
        let version = lease.version.clone();
        *lease = leases.save(lease.clone(), &version).await?;
    }
    if lease.state == LeaseState::Ready.as_str() || lease.state == LeaseState::Starting.as_str() {
        match runtime.start(job, lease).await {
            Ok(started) => return Ok(started),
            Err(err) => {
                if !lease.operation_started_at.is_empty() {
                    if let Some(started) = parse_iso(&lease.operation_started_at) {
                        let age = (Utc::now() - started).num_seconds();
                        if age >= START_RECOVERY_SECONDS {
                            runtime
                                .fail(job, lease, "Box start did not recover before deadline", false)
                                .await?;
                            return Ok(true);
                        }
                    }
                }
                return Err(err);
            }
        }
    }
    if lease.state == LeaseState::Running.as_str() {
        return runtime.reconcile_running(job, lease).await;
    }
    if lease.state == LeaseState::Allocating.as_str() {
        runtime
            .fail(job, lease, "Box allocation did not record a resource", true)
            .await?;
        return Ok(true);
    }
    Err(BoxDispatchError::runtime(format!("unhandled Box lease state {}", lease.state)))
}

/// Python `reconcile_box_jobs`: advance every persisted lease state
/// without duplicating mutations.
pub async fn reconcile_box_jobs(
    store: &JobStorage,
    provider: &BoxProvider,
    owner_id: &str,
) -> Result<i64, BoxDispatchError> {
    let leases = ProviderLeaseStore::new(store.clone());
    let runtime = BoxRuntime::new(store, provider, &leases);
    let mut changed: i64 = 0;
    for mut job in store.list_jobs("running", 0).await? {
        if !matches!(job.provider.as_str(), "box" | "box-ascii") {
            continue;
        }
        let ttl = if job.box_ttl_seconds != 0 { job.box_ttl_seconds } else { provider.ttl_seconds };
        let mut lease = match leases
            .acquire(&job.job_id, "box", owner_id, OWNER_TTL_SECONDS, ttl)
            .await
        {
            Ok(lease) => Some(lease),
            Err(err) if err.is_conflict() => continue,
            Err(err) => {
                log_failure(&job.job_id, &err.into());
                continue;
            }
        };
        if let Some(l) = lease.as_mut() {
            match reconcile_one(provider, &runtime, &leases, &mut job, l).await {
                Ok(true) => changed += 1,
                Ok(false) => {}
                Err(err) if err.is_conflict() => {}
                Err(err) => log_failure(&job.job_id, &err),
            }
        }
        relinquish(&leases, lease).await;
    }
    Ok(changed)
}

/// Python `cancel_box_job`: cancel the process or prompt, then release
/// the fenced resource.
pub async fn cancel_box_job(
    store: &JobStorage,
    provider: &BoxProvider,
    job: &mut Job,
    owner_id: &str,
) -> Result<(), BoxDispatchError> {
    let leases = ProviderLeaseStore::new(store.clone());
    let session_owner = format!("{owner_id}:{}", Uuid::new_v4().simple());
    let ttl = if job.box_ttl_seconds != 0 { job.box_ttl_seconds } else { provider.ttl_seconds };
    let mut lease = leases
        .acquire(&job.job_id, "box", &session_owner, OWNER_TTL_SECONDS, ttl)
        .await?;
    let runtime = BoxRuntime::new(store, provider, &leases);
    let result = runtime.cancel(job, &mut lease).await;
    relinquish(&leases, Some(lease)).await;
    result
}

/// Python `cancel_box_for_legacy_move`: the fenced bridge used by
/// `BoxProvider.delete_instance` when a running/ job still references the
/// box. NOTE: Python does NOT relinquish here (no finally) — the owner
/// TTL lapses on its own.
pub async fn cancel_box_for_legacy_move(
    store: &JobStorage,
    provider: &BoxProvider,
    job: &mut Job,
    owner_id: &str,
) -> Result<(), BoxDispatchError> {
    let leases = ProviderLeaseStore::new(store.clone());
    let session_owner = format!("{owner_id}:{}", Uuid::new_v4().simple());
    let ttl = if job.box_ttl_seconds != 0 { job.box_ttl_seconds } else { provider.ttl_seconds };
    let mut lease = leases
        .acquire(&job.job_id, "box", &session_owner, OWNER_TTL_SECONDS, ttl)
        .await?;
    let runtime = BoxRuntime::new(store, provider, &leases);
    runtime.interrupt(job, &lease).await?;
    lease.result_state = job_state::FAILED.to_string();
    lease.last_error = "cancelled".to_string();
    let terminal = [
        LeaseState::Failed.as_str(),
        LeaseState::Releasing.as_str(),
        LeaseState::Released.as_str(),
    ];
    if !terminal.contains(&lease.state.as_str()) {
        let token = lease.fence_token.clone();
        lease.transition(LeaseState::Failed, &session_owner, &token)?;
        let version = lease.version.clone();
        lease = leases.save(lease, &version).await?;
    }
    if lease.state == LeaseState::Failed.as_str() {
        let token = lease.fence_token.clone();
        lease.transition(LeaseState::Releasing, &session_owner, &token)?;
        let version = lease.version.clone();
        lease = leases.save(lease, &version).await?;
    }
    if lease.state == LeaseState::Releasing.as_str() {
        provider.release_box(&lease.provider_resource_id).await?;
        let token = lease.fence_token.clone();
        lease.transition(LeaseState::Released, &session_owner, &token)?;
        let version = lease.version.clone();
        leases.save(lease, &version).await?;
    }
    Ok(())
}

/// Python `run_box_tick`: unique owner per invocation; reconcile before
/// allocating.
pub async fn run_box_tick(
    store: &JobStorage,
    provider: &BoxProvider,
    owner_id: &str,
) -> Result<i64, BoxDispatchError> {
    let session_owner = format!("{owner_id}:{}", Uuid::new_v4().simple());
    let reconciled = reconcile_box_jobs(store, provider, &session_owner).await?;
    let dispatched = dispatch_box_jobs(store, provider, &session_owner).await?;
    Ok(reconciled + dispatched)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::r#box::{BoxClient, BoxHttpTransport};
    use crate::queue::local_file::LocalBackend;
    use crate::testutil::{http_response, mock_http};
    use std::sync::Arc;

    const BX: &str = "bx_2abcdefg";

    fn store() -> (tempfile::TempDir, JobStorage) {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(dir.path().to_str().unwrap()).unwrap();
        (dir, JobStorage::with_backend(Arc::new(backend), "local"))
    }

    fn box_job(job_id: &str) -> Job {
        let mut job = Job::new(job_id, "echo hi");
        job.provider = "box".into();
        job.pin_to_provider = true;
        job
    }

    fn provider_for(base_url: &str) -> BoxProvider {
        let transport = BoxHttpTransport::new_for_test("box_testkey", base_url, 5.0);
        BoxProvider::new(BoxClient::from_transport(transport), 7200).unwrap()
    }

    fn box_info(state: &str) -> String {
        http_response(
            200,
            "OK",
            &format!(r#"{{"ok": true, "type": "box.info", "box": {{"id": "{BX}", "state": "{state}"}}}}"#),
        )
    }

    /// Full lease machine: ALLOCATING -> PROVISIONING -> READY -> STARTING
    /// -> RUNNING -> COLLECTING -> RELEASING -> RELEASED over three
    /// run_box_tick invocations, with the job landing in completed/ and
    /// bounded logs uploaded to status/.
    #[tokio::test]
    async fn run_box_tick_drives_full_lease_state_machine() {
        let (_dir, store) = store();
        store.write_job("queue", &box_job("j1")).await.unwrap();

        let server = mock_http(vec![
            // Tick 1 dispatch: preflight + create_box.
            http_response(200, "OK", r#"{"ok": true, "type": "limits.info", "canStart": true}"#),
            http_response(
                200,
                "OK",
                r#"{"ok": true, "type": "box.created", "box": {"id": "bx_2abcdefg", "state": "provisioning"}}"#,
            ),
            // Tick 2 reconcile: get_box(ready), renew, write run.sh, launch.
            box_info("ready"),
            http_response(200, "OK", r#"{"ok": true, "type": "box.updated", "box": {"id": "bx_2abcdefg"}}"#),
            http_response(200, "OK", r#"{"ok": true, "type": "file.written"}"#),
            http_response(
                200,
                "OK",
                r#"{"ok": true, "type": "command.finished", "success": true, "exitCode": 0}"#,
            ),
            // Tick 3 reconcile: get_box(running), renew, exit=0, stdout,
            // stderr (404), release (get_box + stop).
            box_info("running"),
            http_response(200, "OK", r#"{"ok": true, "type": "box.updated", "box": {"id": "bx_2abcdefg"}}"#),
            http_response(200, "OK", r#"{"ok": true, "type": "file.read", "content": "0"}"#),
            http_response(200, "OK", r#"{"ok": true, "type": "file.read", "content": "hello world"}"#),
            http_response(404, "Not Found", r#"{"code": "file_not_found", "message": "no"}"#),
            box_info("running"),
            http_response(200, "OK", r#"{"ok": true, "type": "box.stopping"}"#),
        ])
        .await;
        let provider = provider_for(&server.base_url);

        // Tick 1: dispatch admits the pinned box job and allocates a box.
        let n = run_box_tick(&store, &provider, "coord").await.unwrap();
        assert_eq!(n, 1);
        let running = store.read_job("running", "j1").await.unwrap().expect("job in running/");
        assert_eq!(running.instance_ref.as_deref(), Some(BX));
        let leases = ProviderLeaseStore::new(store.clone());
        let lease = leases.load("j1").await.unwrap().unwrap();
        assert_eq!(lease.state, "provisioning");
        assert_eq!(lease.provider_resource_id, BX);

        // Tick 2: provisioning -> ready -> starting -> running (launch).
        let n = run_box_tick(&store, &provider, "coord").await.unwrap();
        assert_eq!(n, 1);
        let lease = leases.load("j1").await.unwrap().unwrap();
        assert_eq!(lease.state, "running");
        assert!(lease.operation_id.starts_with("stado-j1"));

        // Tick 3: exit file present -> logs upload -> collecting ->
        // releasing -> released; job completes.
        let n = run_box_tick(&store, &provider, "coord").await.unwrap();
        assert_eq!(n, 1);
        let lease = leases.load("j1").await.unwrap().unwrap();
        assert_eq!(lease.state, "released");
        assert_eq!(lease.result_state, "completed");
        let completed = store.read_job("completed", "j1").await.unwrap().expect("job in completed/");
        assert!(completed.completed_at.is_some());
        assert!(store.read_job("running", "j1").await.unwrap().is_none());
        let stdout =
            store.download_text("status/j1/output/command_stdout.log").await.unwrap().unwrap();
        assert_eq!(stdout, "hello world");
        let stderr =
            store.download_text("status/j1/output/command_stderr.log").await.unwrap().unwrap();
        assert_eq!(stderr, "");

        // Endpoint-level assertions over the recorded request sequence.
        let requests = server.requests.lock().unwrap().clone();
        server.stop();
        assert_eq!(requests.len(), 13, "{requests:?}");
        assert!(requests[0].starts_with("GET /limits "), "{}", requests[0]);
        assert!(requests[1].starts_with("POST /boxes "), "{}", requests[1]);
        assert!(requests[2].starts_with("GET /boxes/bx_2abcdefg "), "{}", requests[2]);
        assert!(requests[3].starts_with("PATCH /boxes/bx_2abcdefg "), "{}", requests[3]);
        assert!(requests[4].starts_with("PUT /boxes/bx_2abcdefg/files "), "{}", requests[4]);
        // run.sh contents: the idempotent command wrapper.
        assert!(requests[4].contains(".stado/j1/run.sh"), "{}", requests[4]);
        assert!(requests[5].starts_with("POST /boxes/bx_2abcdefg/commands "), "{}", requests[5]);
        // The launch shell is exit-file/marker/pid guarded (idempotent).
        assert!(requests[5].contains("launch_intent"), "{}", requests[5]);
        assert!(requests[8].contains("exit_code"), "{}", requests[8]);
        assert!(requests[9].contains("stdout.log"), "{}", requests[9]);
        assert!(requests[10].contains("stderr.log"), "{}", requests[10]);
        assert!(requests[12].starts_with("POST /boxes/bx_2abcdefg/stop "), "{}", requests[12]);
    }

    /// A queued job pinned to box that fails admission is moved to
    /// failed/ with the joined reasons (no lease is created).
    #[tokio::test]
    async fn dispatch_fails_unpinned_and_inadmissible_jobs() {
        let (_dir, store) = store();
        let server = mock_http(vec![]).await;
        let provider = provider_for(&server.base_url);

        let mut unpinned = box_job("j-unpinned");
        unpinned.pin_to_provider = false;
        store.write_job("queue", &unpinned).await.unwrap();
        let mut gpu = box_job("j-gpu");
        gpu.gpu_mem_gb = 16;
        store.write_job("queue", &gpu).await.unwrap();

        let n = dispatch_box_jobs(&store, &provider, "coord").await.unwrap();
        assert_eq!(n, 0);
        let failed = store.read_job("failed", "j-unpinned").await.unwrap().unwrap();
        assert_eq!(failed.error.as_deref(), Some("Box jobs must set pin_to_provider=true"));
        let failed = store.read_job("failed", "j-gpu").await.unwrap().unwrap();
        assert_eq!(failed.error.as_deref(), Some("target has no accelerator"));
        // Nothing was allocated; no HTTP calls, no leases.
        assert!(server.requests.lock().unwrap().is_empty());
        server.stop();
        let leases = ProviderLeaseStore::new(store.clone());
        assert!(leases.load("j-gpu").await.unwrap().is_none());
    }

    /// A box that vanishes mid-run fails the job with resource_released
    /// semantics (lease ends released, job in failed/).
    #[tokio::test]
    async fn gone_box_fails_job_and_releases_lease() {
        let (_dir, store) = store();
        let mut job = box_job("j2");
        job.state = job_state::RUNNING.into();
        job.instance_ref = Some(BX.into());
        store.write_job("running", &job).await.unwrap();
        // Pre-seed a RUNNING lease owned by a long-dead owner so reconcile
        // can take it over (fields set directly: the fenced transitions
        // refuse an already-expired owner).
        let leases = ProviderLeaseStore::new(store.clone());
        let mut lease = ProviderLease::new("j2", "box", "dead-owner", 0, 7200);
        lease.provider_resource_id = BX.into();
        lease.state = LeaseState::Running.as_str().to_string();
        let lease = leases.create(lease).await.unwrap();
        assert!(lease.owner_expired().unwrap());

        let server = mock_http(vec![
            // renew_box PATCH (state is RUNNING -> renewed).
            http_response(404, "Not Found", r#"{"code": "box_not_found", "message": "gone"}"#),
        ])
        .await;
        let provider = provider_for(&server.base_url);
        // box_state maps the 404 to "gone" -> fail(resource_released=true)
        // drives the lease straight to RELEASED without a provider call.
        let n = reconcile_box_jobs(&store, &provider, "coord").await.unwrap();
        assert_eq!(n, 1);
        server.stop();
        let lease = leases.load("j2").await.unwrap().unwrap();
        assert_eq!(lease.state, "released");
        assert_eq!(lease.result_state, "failed");
        assert!(lease.last_error.contains("Box became gone before completion"));
        let failed = store.read_job("failed", "j2").await.unwrap().unwrap();
        assert_eq!(
            failed.error.as_deref(),
            Some("Box became gone before completion")
        );
    }
}
