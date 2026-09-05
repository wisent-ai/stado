//! Phantom-job reaper: provider-neutral recovery of jobs whose worker died.
//!
//! Distinct from the two existing reapers:
//! - [`crate::monitor::monitor`] (`check_running_jobs` / `reap_dead_agents`)
//!   runs per CLOUD provider arm of the coordinator tick and deletes dead
//!   agent VMs. A fleet with no cloud arm (local/box workers only, or a
//!   provider API outage failing the arm) never runs it, so a worker that
//!   dies mid-job leaves the job in `running/` forever — phantom capacity
//!   the scheduler keeps counting. Confirmed live 2026-08-19/20: two jobs
//!   sat in `running` with no live worker behind them for hours.
//! - [`crate::monitor::reap`] deletes per-job blobs of fully-terminal runs
//!   and never touches live records.
//!
//! This reaper keys on the job's worker lease, and the lease lives IN the
//! running job document (`Job::lease_expires_at`), renewed by
//! [`crate::queue::storage::JobStorage::renew_running_lease`] every
//! [`crate::providers::local::slots::HEARTBEAT_INTERVAL_S`] from
//! `write_heartbeat`. The TTL is the codebase's own
//! [`crate::config::HEARTBEAT_STALE_MINUTES`] — the window
//! [`super::control::default_drain_timeout_s`] documents as "the window
//! after which the monitor declares a running job's heartbeat dead".
//!
//! Why in the document: while the lease was only the `status/<job_id>/heartbeat`
//! blob, no amount of re-reading could fence this reaper. It read the running
//! job at version V, read the pulse, and moved the job at V; a live worker
//! that refreshed its pulse in that window changed nothing the reaper held,
//! so the move succeeded, the job was requeued and a second worker started it
//! while the first was still executing and about to publish its result. A
//! renewal that is a compare-and-swap on the running document invalidates V,
//! so the reaper's version-pinned move fails and it loses the race instead of
//! silently winning it. Jobs claimed before the lease existed carry none, and
//! for exactly those the heartbeat blob and `started_at` still decide, with a
//! re-read immediately before the move.
//!
//! Retry semantics: a stale release worker whose complete canonical receipt and
//! archive still verify against its immutable request is moved directly to
//! `completed/`; the evidence is the result, so rebuilding it would throw away
//! a successful qualification. Every other first lease expiry moves the job
//! back to `queue/` exactly once, incrementing the existing `restarts` retry
//! field (still bounded by `max_restarts`) and storing
//! [`LEASE_EXPIRED_REASON`] in `job.error` — both the diagnosis readers surface
//! and the marker that a second expiry turns the job `failed/` with that same
//! stored reason.
//!
//! Write discipline: every transition uses
//! [`JobStorage::move_job_if_version`], which persists explicit ownership and
//! source-generation intent, CAS-fences the source, then creates or validates
//! the destination. Any caller can finish an abandoned transition.

use chrono::Utc;

use crate::config;
use crate::models::{isoformat_utc, job_state, Job};
use crate::monitor::heartbeat_guard as hg;

use super::{capacity, JobStorage, StorageError};

/// Stored `job.error` reason for a lease-expiry transition. Written on the
/// first-expiry requeue (where it doubles as the "already requeued once"
/// marker) and kept as the terminal reason on the second-expiry failure.
pub const LEASE_EXPIRED_REASON: &str = "worker lease expired";

/// Checkpoint-freshness defer window, the same 5400s the monitor's orphan
/// branch uses: a multi-GB checkpoint upload can starve the heartbeat PUT
/// while the job is demonstrably alive (2026-05-16/17 incidents).
const CHECKPOINT_FRESH_SECONDS: f64 = 5400.0;

/// One tick's worth of reaper work.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReaperSummary {
    /// Running jobs moved back to `queue/` on their first lease expiry.
    pub requeued: usize,
    /// Running jobs moved to `failed/` on their second lease expiry (or
    /// with the restart budget already spent).
    pub failed: usize,
    /// Release jobs completed from a verified canonical receipt and archive
    /// after their worker lease expired.
    pub release_completions: usize,
    /// Queued jobs whose `assigned_to` named a silent worker, cleared so
    /// another worker can claim them.
    pub assignments_cleared: usize,
    /// Whether the marker-index sweep has covered `queue/` end-to-end at
    /// least once, which is what lets the listing walk drop its
    /// whole-prefix fallback. Reported, not acted on, here.
    pub index_swept: bool,
}

/// Seconds since `status/<job_id>/heartbeat` was last written, or `None`
/// when no heartbeat blob exists (a worker that died before its first
/// write, or a just-requeued job whose status blobs were cleaned).
async fn heartbeat_age_seconds(
    store: &JobStorage,
    job_id: &str,
    now: chrono::DateTime<Utc>,
) -> Result<Option<i64>, StorageError> {
    let path = format!("status/{job_id}/heartbeat");
    Ok(store
        .backend()
        .updated_at(&path)
        .await?
        .map(|updated| (now - updated).num_seconds()))
}

/// Seconds since the job was (last) started, per its `started_at` stamp.
fn started_age_seconds(job: &Job, now: chrono::DateTime<Utc>) -> Option<i64> {
    let started = hg::parse_iso_lenient(job.started_at.as_deref().filter(|s| !s.is_empty())?)?;
    Some((now - started).num_seconds())
}
/// Return the completion timestamp when this stale job has a complete,
/// self-consistent release-worker result in canonical storage.
///
/// The bootstrap writes the canonical archive and receipt only after the
/// release worker exits successfully. The receipt is still not trusted by
/// itself: its immutable request is read through the job's resolved input,
/// both digests are checked, and every identity field must agree. Requiring
/// the receipt timestamp to belong to this exact execution prevents output
/// retained from an earlier lease-expiry retry from completing a newer one.
async fn verified_release_completion(
    store: &JobStorage,
    job: &Job,
    now: chrono::DateTime<Utc>,
    log: &dyn Fn(&str),
) -> Result<Option<String>, StorageError> {
    let receipt_path = format!("status/{}/output/receipt.json", job.job_id);
    let Some(receipt_bytes) = store.read_bytes(&receipt_path).await? else {
        return Ok(None);
    };
    let receipt: crate::release_pipeline::BuildReceipt =
        match serde_json::from_slice(&receipt_bytes) {
            Ok(receipt) => receipt,
            Err(error) => {
                log(&format!(
                    "{}: retained output is not a release receipt: {error}",
                    job.job_id
                ));
                return Ok(None);
            }
        };
    let Some(request_input) = job
        .resolved_input_artifacts
        .get("request")
        .and_then(serde_json::Value::as_object)
    else {
        return Ok(None);
    };
    if request_input
        .get("relative_path")
        .and_then(serde_json::Value::as_str)
        != Some("release-request.json")
    {
        return Ok(None);
    }
    let Some(request_uri) = request_input
        .get("stado_uri")
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(None);
    };
    let Some(request_sha256) = request_input
        .get("sha256")
        .and_then(serde_json::Value::as_str)
    else {
        return Ok(None);
    };
    let request_object = match crate::object_store::ObjectRef::parse(request_uri) {
        Ok(object) => object,
        Err(error) => {
            log(&format!(
                "{}: release request URI is invalid: {error}",
                job.job_id
            ));
            return Ok(None);
        }
    };
    let configured_namespace = crate::config::wc_stado_storage_namespace();
    if !configured_namespace.is_empty() && request_object.namespace() != configured_namespace {
        log(&format!(
            "{}: release request namespace {} differs from queue namespace {}",
            job.job_id,
            request_object.namespace(),
            configured_namespace
        ));
        return Ok(None);
    }
    let request_path = store.backend().blob_path(&request_object);
    let Some(request_bytes) = store.read_bytes(&request_path).await? else {
        log(&format!(
            "{}: release request disappeared from {request_uri}",
            job.job_id
        ));
        return Ok(None);
    };
    if crate::release_control::sha256_bytes(&request_bytes) != request_sha256 {
        log(&format!(
            "{}: release request digest disagrees with its immutable job input",
            job.job_id
        ));
        return Ok(None);
    }
    let request: crate::release_pipeline::WorkerRequest =
        match serde_json::from_slice(&request_bytes) {
            Ok(request) => request,
            Err(error) => {
                log(&format!(
                    "{}: immutable release request is invalid: {error}",
                    job.job_id
                ));
                return Ok(None);
            }
        };
    let completed = match hg::parse_iso_lenient(&receipt.completed_at) {
        Some(completed) => completed,
        None => {
            log(&format!(
                "{}: release receipt has an invalid completion timestamp",
                job.job_id
            ));
            return Ok(None);
        }
    };
    let started = job
        .started_at
        .as_deref()
        .filter(|value| !value.is_empty())
        .and_then(hg::parse_iso_lenient);
    let inputs_match = receipt.inputs.len() == request.inputs.len()
        && receipt.inputs.iter().all(|(name, input)| {
            request.inputs.get(name).is_some_and(|expected| {
                input.uri == expected.uri
                    && input.sha256 == expected.sha256
                    && input.mount == expected.mount
                    && input.extract == expected.extract
            })
        });
    let identity_matches = receipt.schema_version == 1
        && request.schema_version == 1
        && receipt.run_id == request.run_id
        && receipt.job_id == job.job_id
        && receipt.product == request.product
        && receipt.version == request.version
        && receipt.platform == request.platform
        && receipt.builder == request.builder
        && receipt.source_commit == request.source_commit
        && receipt.source_sha256 == request.source_sha256
        && receipt.manifest_sha256 == request.manifest_sha256
        && receipt.secret_env == request.secret_env
        && inputs_match
        && receipt.status == crate::release_pipeline::StepStatus::Passed
        && receipt.build.status == crate::release_pipeline::StepStatus::Passed
        && receipt
            .quality
            .iter()
            .all(|step| step.status == crate::release_pipeline::StepStatus::Passed)
        && started.is_some_and(|started| completed >= started)
        && completed <= now;
    if !identity_matches {
        log(&format!(
            "{}: retained release receipt does not match this execution's immutable request",
            job.job_id
        ));
        return Ok(None);
    }
    let Some(artifact) = receipt.artifact.as_ref() else {
        log(&format!(
            "{}: passed release receipt omitted its artifact",
            job.job_id
        ));
        return Ok(None);
    };
    if artifact.path != "release.tar.gz" {
        log(&format!(
            "{}: release receipt names unexpected artifact path {:?}",
            job.job_id, artifact.path
        ));
        return Ok(None);
    }
    let archive_path = format!("status/{}/output/release.tar.gz", job.job_id);
    let Some(archive) = store.read_bytes(&archive_path).await? else {
        log(&format!(
            "{}: passed release receipt has no canonical archive",
            job.job_id
        ));
        return Ok(None);
    };
    if u64::try_from(archive.len()).ok() != Some(artifact.bytes)
        || crate::release_control::sha256_bytes(&archive) != artifact.sha256
    {
        log(&format!(
            "{}: canonical release archive disagrees with its receipt",
            job.job_id
        ));
        return Ok(None);
    }
    Ok(Some(receipt.completed_at))
}

/// Reap one running job whose lease is expired: requeue on the first
/// expiry, fail on the second. Leaves the job alone (fresh, guarded, or
/// the fence lost to a concurrent writer) without counting it.
async fn reap_one(
    store: &JobStorage,
    job_id: &str,
    lease_ttl_seconds: i64,
    now: chrono::DateTime<Utc>,
    log: &dyn Fn(&str),
    summary: &mut ReaperSummary,
) -> Result<(), StorageError> {
    store.recover_job_transition(job_id).await?;
    let path = format!("running/{job_id}.json");
    let Some(versioned) = store.read_text_versioned(&path).await? else {
        return Ok(()); // already moved by a concurrent writer
    };
    let mut job = Job::from_json(&versioned.content)?;
    if job.state != job_state::RUNNING {
        store.recover_job_transition(job_id).await?;
        return Ok(());
    }
    // The lease the worker renews INSIDE this document is the authority when
    // the document carries one. It is also the fence: the renewal is a
    // compare-and-swap on this very object, so a pulse that lands between
    // this read and the move below changes `versioned.version` and the
    // version-pinned move fails. That is the only construction that closes
    // the race — re-reading a heartbeat blob written beside the job cannot,
    // because nothing the reaper pins changes when it is written.
    //
    // A job claimed before the lease existed carries none. For those the old
    // signals still decide: the heartbeat blob when one exists, and
    // `started_at` as boot grace. An undateable job is skipped rather than
    // reaped on an invented fact.
    let lease_expiry = job
        .lease_expires_at
        .as_deref()
        .filter(|value| !value.is_empty())
        .and_then(hg::parse_iso_lenient);
    let heartbeat_age = heartbeat_age_seconds(store, job_id, now).await?;
    let started_age = started_age_seconds(&job, now);
    let fresh = |age: Option<i64>| age.is_some_and(|age| age <= lease_ttl_seconds);
    let age = match lease_expiry {
        Some(expires) => {
            if expires > now {
                return Ok(());
            }
            // An expired lease beside a FRESH pulse means the renewal write
            // is failing, not that the worker died: both come from the same
            // `write_heartbeat`, so a live executor whose compare-and-swap
            // keeps losing must not be reaped for the storage layer's fault.
            if fresh(heartbeat_age) {
                return Ok(());
            }
            (now - expires).num_seconds() + lease_ttl_seconds
        }
        None => {
            if heartbeat_age.is_none() && started_age.is_none() {
                return Ok(());
            }
            if fresh(heartbeat_age) || fresh(started_age) {
                return Ok(());
            }
            // The freshest (smallest) stale age, named in the log line.
            [heartbeat_age, started_age]
                .into_iter()
                .flatten()
                .min()
                .unwrap_or_default()
        }
    };

    // A command that kills `wc agent` itself is done, not orphaned: the
    // agent's disappearance is the success condition.
    if hg::finalize_if_self_terminating(store, &mut job, log).await? {
        return Ok(());
    }
    // A fresh checkpoint write is proof of life immune to the heartbeat
    // starvation a multi-GB upload causes; defer to it.
    if hg::any_job_checkpoint_fresh(store, &job, CHECKPOINT_FRESH_SECONDS).await {
        return Ok(());
    }

    // A job with no in-document lease has no fence at all, so every external
    // signal it does have is re-read immediately before the move: a worker
    // that refreshed its heartbeat or checkpoint while the finalizer and
    // first checkpoint inspection above were running must not be reaped from
    // the stale observation. A lease-bearing job needs none of this — its
    // renewal invalidates the version the move below is pinned to.
    if lease_expiry.is_none()
        && (fresh(heartbeat_age_seconds(store, job_id, now).await?)
            || hg::any_job_checkpoint_fresh(store, &job, CHECKPOINT_FRESH_SECONDS).await)
    {
        return Ok(());
    }

    // The worker can finish and durably publish its complete result just before
    // the owning agent is replaced. In that state retrying the build is both
    // wasteful and wrong: the immutable qualification already exists. This
    // runs only after the same stale-lease checks that protect every live job,
    // and the version-pinned transition below still loses to any late renewal.
    if let Some(completed_at) = verified_release_completion(store, &job, now, log).await? {
        job.state = job_state::COMPLETED.to_string();
        job.completed_at = Some(completed_at);
        job.failed_at = None;
        job.error = None;
        if store
            .move_job_if_version(&job, "running", "completed", &versioned.version)
            .await?
        {
            summary.release_completions += 1;
            log(&format!(
                "{job_id}: completed from verified durable release output after worker lease expiry"
            ));
        }
        return Ok(());
    }
    let second_expiry = job.error.as_deref() == Some(LEASE_EXPIRED_REASON);
    if second_expiry || job.restarts + 1 > job.max_restarts {
        job.state = job_state::FAILED.to_string();
        job.failed_at = Some(isoformat_utc(now));
        job.error = Some(LEASE_EXPIRED_REASON.to_string());
        if store
            .move_job_if_version(&job, "running", "failed", &versioned.version)
            .await?
        {
            summary.failed += 1;
            let why = if second_expiry {
                "second lease expiry".to_string()
            } else {
                format!("restart cap {} exceeded", job.max_restarts)
            };
            log(&format!(
                "{job_id}: FAILED ({LEASE_EXPIRED_REASON}; lease silent for {age}s; {why})"
            ));
        }
        return Ok(());
    }

    job.restarts += 1;
    job.state = job_state::QUEUED.to_string();
    job.instance_ref = None;
    job.started_at = None;
    job.last_restart = Some(isoformat_utc(now));
    job.error = Some(LEASE_EXPIRED_REASON.to_string());
    // The worker that held the lease is dead; leaving its name in
    // assigned_to would pin the job to phantom capacity (job_eligible
    // rejects every other claimant). Empty assigned_to is the documented
    // any-eligible-agent semantic. An operator hard-pin (pinned_host) is
    // the exception: assigned_to mirrors it, and
    // repair_conflicting_pinned_assignments restores the mirror anyway.
    if job.pinned_host.is_empty() {
        job.assigned_to = String::new();
    }
    if store
        .move_job_if_version(&job, "running", "queue", &versioned.version)
        .await?
    {
        summary.requeued += 1;
        store.cleanup_status(&job.job_id).await?;
        log(&format!(
            "{job_id}: requeued ({LEASE_EXPIRED_REASON}; lease silent for {age}s; restart {}/{})",
            job.restarts, job.max_restarts
        ));
    }
    Ok(())
}

/// Clear `assigned_to` on queued jobs whose named worker has gone silent,
/// so another worker can claim them. Silence is the codebase's own
/// liveness horizon: absence from [`capacity::read_consumer_capacity`],
/// which drops every publication older than
/// [`capacity::CAPACITY_STALE_SECONDS`]. Operator hard-pins
/// (`pinned_host`) are never touched — the same rule the makespan matcher
/// follows.
async fn clear_silent_assignments(
    store: &JobStorage,
    now: chrono::DateTime<Utc>,
    log: &dyn Fn(&str),
    summary: &mut ReaperSummary,
) -> Result<(), StorageError> {
    let live = capacity::read_consumer_capacity(store).await?;
    for candidate in store.list_jobs("queue", 0).await? {
        store.recover_job_transition(&candidate.job_id).await?;
        let Some(current) = store.read_job("queue", &candidate.job_id).await? else {
            continue;
        };
        if current.state != job_state::QUEUED {
            continue;
        }
        if current.assigned_to.is_empty() || !current.pinned_host.is_empty() {
            continue;
        }
        let worker_live = live
            .keys()
            .any(|consumer| consumer.eq_ignore_ascii_case(&current.assigned_to));
        if worker_live {
            continue;
        }
        let job_id = current.job_id.clone();
        let path = format!("queue/{job_id}.json");
        let Some(versioned) = store.read_text_versioned(&path).await? else {
            continue;
        };
        let mut job = Job::from_json(&versioned.content)?;
        if job.state != job_state::QUEUED || job.assigned_to != current.assigned_to {
            continue; // changed under the listing; the next tick reconciles
        }
        let worker = std::mem::take(&mut job.assigned_to);
        match store
            .compare_and_swap_text(&path, &versioned.version, &job.to_json())
            .await
        {
            Ok(_) => {
                summary.assignments_cleared += 1;
                let broadcast = store
                    .backend()
                    .updated_at(&format!("{}{worker}.json", capacity::CAPACITY_PREFIX))
                    .await?;
                let silence = match broadcast {
                    Some(updated) => {
                        format!("last broadcast {}s ago", (now - updated).num_seconds())
                    }
                    None => "no capacity broadcast on record".to_string(),
                };
                log(&format!(
                    "{job_id}: cleared assignment to silent worker {worker} ({silence})"
                ));
            }
            Err(StorageError::StorageConflict(_)) => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// One reaper pass over the queue: repair the marker index, recover phantom
/// running jobs, then release queued jobs pinned to silent workers. Called
/// from the coordinator tick before assignment so recovered work is
/// dispatchable in the same tick.
pub async fn reap_expired_leases(
    store: &JobStorage,
    log: &dyn Fn(&str),
) -> Result<ReaperSummary, StorageError> {
    let now = Utc::now();
    let lease_ttl_seconds = config::HEARTBEAT_STALE_MINUTES * 60;
    // The index repair belongs on this tick, and it never retires. A queued
    // job whose marker write did not land is invisible to every scheduler
    // while still reporting `queued` — the same class of stranding this
    // reaper exists to undo, just on the listing index instead of the lease.
    // The sweep is bounded per call and its cursor wraps, so this is a fixed
    // cost per tick that eventually re-examines every queued job rather than
    // a one-shot migration that stops looking. It runs before the passes
    // below so a recovered marker is claimable in this same tick.
    let index_swept =
        crate::queue::migrations::backfill_priority_markers(store, config::MARKER_REPAIR_PER_TICK)
            .await?;
    let mut summary = ReaperSummary {
        index_swept,
        ..Default::default()
    };
    for candidate in store.list_jobs("running", 0).await? {
        if candidate.job_id.is_empty() {
            continue;
        }
        reap_one(
            store,
            &candidate.job_id,
            lease_ttl_seconds,
            now,
            log,
            &mut summary,
        )
        .await?;
    }
    clear_silent_assignments(store, now, log, &mut summary).await?;
    Ok(summary)
}
