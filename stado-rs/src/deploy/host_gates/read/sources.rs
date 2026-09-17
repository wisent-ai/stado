//! The two store reads behind the verdict: which publication is this host's,
//! and what work is pinned to it.

use chrono::{DateTime, Utc};

use crate::deploy::host_gates::gates::WaitingJob;
use crate::deploy::DeployError;
use crate::queue::capacity::{self, Publication};
use crate::queue::JobStorage;
use crate::targets::{ComputeTarget, Registry};

/// How many pinned jobs one gates read returns, and how many queued
/// documents it may download to find them. The read walks the priority
/// index — `queue_priority/`, which names queued jobs and nothing else — the
/// same way a worker poll does, so a `queue/` prefix full of settled
/// transition sentinels costs it nothing. On 2026-09-17 that prefix held 779
/// objects for 15 queued jobs, the old whole-prefix download took every
/// host's gates read past its ten-second budget, and every verdict was
/// `claiming: unknown`. A read that fills its window says so in
/// `WaitingRead::partial` instead of pretending the answer is whole.
pub const GATES_QUEUE_WINDOW: usize = 200;
pub const GATES_QUEUE_SCAN_BUDGET: usize = 2_000;

/// The bounded answer to "what is pinned here": the jobs found, and the
/// sentence that says the window was full when the answer is not the whole
/// prefix.
pub(super) struct WaitingRead {
    pub jobs: Vec<WaitingJob>,
    pub partial: Option<String>,
}

/// Queued jobs pinned to this host, oldest first.
///
/// A pinned job names its consumer as `<kind>-<hostname>`, and the hostname
/// is the machine's own word for itself, not its registry name — the same
/// gap [`publication`] closes with [`Registry::lookup_self`], closed the same
/// way here. Jobs pinned by exact registry name are honored too, because the
/// operator-facing `--pinned-host` accepts that spelling.
pub(super) async fn waiting_jobs(
    registry: &Registry,
    target: &ComputeTarget,
    store: &JobStorage,
    now: DateTime<Utc>,
) -> Result<WaitingRead, DeployError> {
    let prefix = format!("{}-", target.kind);
    let pinned_here = |job: &crate::models::Job| -> bool {
        !job.pinned_host.is_empty()
            && (job.pinned_host == target.name
                || job
                    .pinned_host
                    .strip_prefix(prefix.as_str())
                    .is_some_and(|identity| {
                        registry
                            .lookup_self(identity)
                            .ok()
                            .flatten()
                            .is_some_and(|found| found.name == target.name)
                    }))
    };
    let queued = store
        .list_claimable_jobs(
            "queue",
            &crate::queue::listing::JobScan {
                want: GATES_QUEUE_WINDOW,
                scan_budget: GATES_QUEUE_SCAN_BUDGET,
                max_gpu_mem_gb: i64::MAX,
                eligible: &pinned_here,
                from_head: true,
            },
        )
        .await
        .map_err(|exc| DeployError(exc.to_string()))?;
    let partial = (queued.len() >= GATES_QUEUE_WINDOW).then(|| {
        format!(
            "{GATES_QUEUE_WINDOW} pinned jobs filled the window; more may be queued for this host"
        )
    });
    let mut waiting = Vec::new();
    for job in queued {
        let age_seconds = DateTime::parse_from_rfc3339(&job.created_at)
            .ok()
            .map(|created| (now - created.with_timezone(&Utc)).num_seconds());
        waiting.push(WaitingJob {
            job_id: job.job_id,
            age_seconds,
        });
    }
    waiting.sort_by_key(|job| std::cmp::Reverse(job.age_seconds));
    Ok(WaitingRead {
        jobs: waiting,
        partial,
    })
}

/// This host's capacity publication, stale ones included.
///
/// [`capacity::read_consumer_capacity`] cannot be used here: it drops
/// everything past the staleness horizon and garbage-collects what is past the
/// GC horizon, which is correct for a scheduler and exactly wrong for the
/// question being asked. A host whose agent went quiet an hour ago is the case
/// this command has to be able to report, not the case it deletes. So the read
/// goes through [`capacity::read_publications`], the one GC-free reader of
/// that prefix, shared with [`super::fleet_claim`] — two readers of
/// `capacity/<consumer>.json` would eventually give two answers to one
/// question, and the operator would believe whichever they ran first.
///
/// The consumer id is `<kind>-<hostname>`
/// ([`crate::providers::local::agent`]), and the hostname a host publishes is
/// its own, which need not be its registry name. So the identity is put back
/// through [`Registry::lookup_self`] — the fleet's one hostname-to-target
/// resolution — and only the row that resolves to THIS target is kept.
///
/// [`super::fleet_claim`]: crate::deploy::fleet_claim
pub(super) async fn publication(
    registry: &Registry,
    target: &ComputeTarget,
    store: &JobStorage,
) -> Result<Option<Publication>, DeployError> {
    let rows = capacity::read_publications(store)
        .await
        .map_err(|exc| DeployError(exc.to_string()))?;
    for (consumer_id, row) in rows {
        if resolves_to(registry, target, &consumer_id)? {
            return Ok(Some(row));
        }
    }
    Ok(None)
}

/// Whether `consumer_id` — a `<kind>-<hostname>` publication key — names
/// `target`.
///
/// Exported for [`super::fleet_claim`], which asks the same question of every
/// declared host at once and must answer it by exactly this rule.
///
/// [`super::fleet_claim`]: crate::deploy::fleet_claim
pub(in crate::deploy) fn resolves_to(
    registry: &Registry,
    target: &ComputeTarget,
    consumer_id: &str,
) -> Result<bool, DeployError> {
    let Some(identity) = consumer_id.strip_prefix(&format!("{}-", target.kind)) else {
        return Ok(false);
    };
    Ok(registry
        .lookup_self(identity)
        .map_err(|exc| DeployError(exc.to_string()))?
        .is_some_and(|found| found.name == target.name))
}
