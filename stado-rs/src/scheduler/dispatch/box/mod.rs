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
    let text: String = exc
        .to_string()
        .replace(['\r', '\n'], " ")
        .chars()
        .take(512)
        .collect();
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
    // Maintenance-mode gate (queue::control). Box dispatch is the OTHER
    // queue/ -> running/ mover in the coordinator tick, so a pause has to
    // stop it too or `stado queue drain --wait` would watch running/ grow
    // while it waits. Only the ADMIT half is gated: run_box_tick's
    // reconcile pass still drives already-leased boxes to completion —
    // the same asymmetry the local agent has between advance_slot and its
    // claim scan.
    let queue_control = crate::queue::control::read(store).await?;
    if queue_control.paused {
        eprintln!(
            "[box] queue paused ({}); admitting no new jobs",
            queue_control.pause_summary()
        );
        return Ok(i64::default());
    }
    let leases = ProviderLeaseStore::new(store.clone());
    let mut scheduled: i64 = 0;
    for mut job in store
        .list_jobs_priority_first("queue", QUEUE_SCAN_CAP)
        .await?
    {
        if !crate::capabilities::ProviderId::Box.matches(&job.provider) {
            continue;
        }
        if !job.secret_env.is_empty() {
            fail_queued(
                store,
                &mut job,
                "Box jobs do not support workload secret references",
            )
            .await?;
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
        let resource_ttl = if job.box_ttl_seconds != 0 {
            job.box_ttl_seconds
        } else {
            provider.ttl_seconds
        };
        let mut lease: Option<ProviderLease> = None;
        let mut resource_recorded = false;
        let outcome: Result<bool, BoxDispatchError> = async {
            let mut acquired = leases
                .acquire(
                    &job.job_id,
                    crate::capabilities::ProviderId::Box.as_str(),
                    owner_id,
                    OWNER_TTL_SECONDS,
                    resource_ttl,
                )
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
        runtime
            .fail(job, lease, "Box lease has no provider resource", true)
            .await?;
        return Ok(true);
    }
    let box_state = box_state(provider, lease).await?;
    if box_state == "gone" || box_state == "archived" {
        runtime
            .fail(
                job,
                lease,
                &format!("Box became {box_state} before completion"),
                true,
            )
            .await?;
        return Ok(true);
    }
    if box_state == "error" {
        runtime
            .fail(job, lease, "Box entered error state", false)
            .await?;
        return Ok(true);
    }
    let ttl = if job.box_ttl_seconds != 0 {
        job.box_ttl_seconds
    } else {
        provider.ttl_seconds
    };
    if renewed_state(&lease.state) {
        provider
            .renew_box(&lease.provider_resource_id, Some(ttl))
            .await?;
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
                                .fail(
                                    job,
                                    lease,
                                    "Box start did not recover before deadline",
                                    false,
                                )
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
    Err(BoxDispatchError::runtime(format!(
        "unhandled Box lease state {}",
        lease.state
    )))
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
        if !crate::capabilities::ProviderId::Box.matches(&job.provider) {
            continue;
        }
        let ttl = if job.box_ttl_seconds != 0 {
            job.box_ttl_seconds
        } else {
            provider.ttl_seconds
        };
        let mut lease = match leases
            .acquire(
                &job.job_id,
                crate::capabilities::ProviderId::Box.as_str(),
                owner_id,
                OWNER_TTL_SECONDS,
                ttl,
            )
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
    let ttl = if job.box_ttl_seconds != 0 {
        job.box_ttl_seconds
    } else {
        provider.ttl_seconds
    };
    let mut lease = leases
        .acquire(
            &job.job_id,
            crate::capabilities::ProviderId::Box.as_str(),
            &session_owner,
            OWNER_TTL_SECONDS,
            ttl,
        )
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
    let ttl = if job.box_ttl_seconds != 0 {
        job.box_ttl_seconds
    } else {
        provider.ttl_seconds
    };
    let mut lease = leases
        .acquire(
            &job.job_id,
            crate::capabilities::ProviderId::Box.as_str(),
            &session_owner,
            OWNER_TTL_SECONDS,
            ttl,
        )
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

