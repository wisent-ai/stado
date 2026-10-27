//! The two store reads behind the verdict: which publication is this host's,
//! and what work is pinned to it.

use chrono::{DateTime, Utc};

use crate::deploy::host_gates::gates::WaitingJob;
use crate::deploy::DeployError;
use crate::queue::capacity::{self, Publication};
use crate::queue::JobStorage;
use crate::targets::{ComputeTarget, Registry};

/// What is pinned here: every queued job pinned to this host. The read walks
/// the priority index — `queue_priority/`, which names queued jobs and nothing
/// else — the same way a worker poll does, so a `queue/` prefix full of
/// settled transition sentinels costs it nothing. No window or scan budget is
/// chosen here, so the answer is always the whole set.
pub(super) struct WaitingRead {
    pub jobs: Vec<WaitingJob>,
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
    // The scan's filter answers a bool; an identity the registry declares on
    // several targets is kept here and refuses the read after the scan,
    // instead of hiding the job it pins as pinned elsewhere.
    let ambiguous: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    let pinned_here = |job: &crate::models::Job| -> bool {
        if job.pinned_host.is_empty() {
            return false;
        }
        if job.pinned_host == target.name {
            return true;
        }
        let Some(identity) = job.pinned_host.strip_prefix(prefix.as_str()) else {
            return false;
        };
        match registry.lookup_self(identity) {
            Ok(found) => found.is_some_and(|found| found.name == target.name),
            Err(error) => {
                if let Ok(mut kept) = ambiguous.lock() {
                    kept.get_or_insert_with(|| {
                        format!(
                            "job {} is pinned to {}: {error}",
                            job.job_id, job.pinned_host
                        )
                    });
                }
                false
            }
        }
    };
    let queued = store
        .list_claimable_jobs(
            "queue",
            &crate::queue::listing::JobScan {
                want: None,
                scan_budget: None,
                max_gpu_mem_gb: i64::MAX,
                eligible: &pinned_here,
                from_head: true,
            },
        )
        .await
        .map_err(DeployError::from)?;
    match ambiguous.into_inner() {
        Ok(None) => {}
        Ok(Some(cause)) => {
            return Err(DeployError(format!(
                "the jobs waiting on {} cannot be told apart from another host's: {cause}",
                target.name
            )))
        }
        Err(poisoned) => {
            return Err(DeployError(format!(
                "the jobs waiting on {} were not read: the scan's filter panicked ({poisoned})",
                target.name
            )))
        }
    }
    let mut waiting = Vec::new();
    for job in queued {
        let age_seconds = match DateTime::parse_from_rfc3339(&job.created_at) {
            Ok(created) => Some((now - created.with_timezone(&Utc)).num_seconds()),
            Err(unparsed) => {
                tracing::warn!(
                    "queued job {} has a created_at {:?} that is no RFC 3339 time, so its \
                     wait is unknown: {unparsed}",
                    job.job_id,
                    job.created_at
                );
                None
            }
        };
        waiting.push(WaitingJob {
            job_id: job.job_id,
            age_seconds,
            declined: None,
        });
    }
    waiting.sort_by_key(|job| std::cmp::Reverse(job.age_seconds));
    Ok(WaitingRead { jobs: waiting })
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
        .map_err(DeployError::from)?;
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
/// declared host at once and must answer it by exactly this rule, and for
/// placement, which reads the schedules pinned to a host by it.
///
/// [`super::fleet_claim`]: crate::deploy::fleet_claim
pub(crate) fn resolves_to(
    registry: &Registry,
    target: &ComputeTarget,
    consumer_id: &str,
) -> Result<bool, DeployError> {
    let Some(identity) = consumer_id.strip_prefix(&format!("{}-", target.kind)) else {
        return Ok(false);
    };
    Ok(registry
        .lookup_self(identity)
        .map_err(DeployError::from)?
        .is_some_and(|found| found.name == target.name))
}
