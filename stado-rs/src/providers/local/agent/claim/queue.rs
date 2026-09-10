//! What this host may claim right now: the bounded claimable-job listing and
//! the one exception disk pressure still admits.

use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use crate::primitives::constants;
use crate::models::Job;
use crate::providers::local::agent::{Step, POLL_INTERVAL_S};
use crate::providers::local::helpers;
use crate::providers::local::slots::{job_system_packages_eligible, ActiveSlot};
use crate::queue::{JobStorage, StorageError};

/// Read the fresh queue documents this tick may admit, newest operator intent
/// first while the host is under its disk watermark.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn claimable(
    store: &JobStorage,
    consumer_id: &str,
    kind: &str,
    gpu_type: &str,
    total_vram_gb: i64,
    free_vram_gb: i64,
    pinned_only: bool,
    pressure_active: bool,
    claim_store_deadline: Instant,
    current_free_bytes: Option<i64>,
    disk_low_bytes: Option<i64>,
    slots: &[ActiveSlot],
    agent_diag: &mut Map<String, Value>,
    log_fn: &mut dyn FnMut(&str),
) -> anyhow::Result<Step<Vec<Job>>> {
    let claim_budget_left = || claim_store_deadline.saturating_duration_since(Instant::now());
    // Centralized assignment writes job.assigned_to on the queue blob, so
    // the listing itself applies this agent's full admission rule: the
    // window must count jobs this host may claim. Counting jobs that
    // merely fit its VRAM meant a fleet whose oldest two thousand fitting
    // jobs were assigned elsewhere handed this agent nothing claimable on
    // every poll, forever, while its own assigned job sat past the window.
    // The re-read below re-applies the rule to the FRESH document, which
    // is a different fact from the listed snapshot.
    //
    // The listing and the re-reads share ONE budget, because together they
    // are this tick's single question -- "what may I claim right now" --
    // and the answer stops being worth the fleet's belief that this host is
    // alive well before a slow store finishes giving it. A lapsed budget
    // claims nothing and publishes again.
    let queued = match tokio::time::timeout(claim_budget_left(), async {
        let listed = store
            .list_claimable_jobs(
                "queue",
                &crate::queue::listing::JobScan {
                    want: super::super::CLAIM_CANDIDATE_WINDOW,
                    scan_budget: super::super::QUEUE_SCAN_BUDGET,
                    max_gpu_mem_gb: free_vram_gb,
                    eligible: &|job| {
                        helpers::job_eligible(
                            job,
                            gpu_type,
                            total_vram_gb,
                            kind,
                            consumer_id,
                            slots.len(),
                            pinned_only,
                        ) && job_system_packages_eligible(job, kind)
                    },
                    // A claim loop wants reachability and so takes the
                    // shared rotation: a job past this poll's window is
                    // reached by a later poll rather than never.
                    from_head: false,
                },
            )
            .await?;
        let mut queued = Vec::with_capacity(listed.len());
        for candidate in listed {
            if let Some(job) = store.read_job("queue", &candidate.job_id).await? {
                queued.push(job);
            }
        }
        Ok::<_, StorageError>(queued)
    })
    .await
    {
        Ok(result) => result?,
        Err(_) => {
            log_fn(&format!(
                "loop: claimable-job read exhausted this tick's {}s store budget; claiming nothing this tick \
                 and publishing again",
                constants::AGENT_CLAIM_STORE_BUDGET_S
            ));
            tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_S)).await;
            return Ok(Step::Done);
        }
    };
    let mut queued = queued;
    queued.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| a.created_at.cmp(&b.created_at))
    });
    if pressure_active {
        queued.retain(|job| {
            job.gpu_mem_gb == 0
                && job.priority == crate::primitives::constants::RELEASE_JOB_PRIORITY
                && !job.run_id.is_empty()
                && !job.pinned_host.is_empty()
                && job.command == crate::primitives::constants::RELEASE_DELIVERY_JOB_COMMAND
                && job
                    .output_uri
                    .starts_with("stado://probierz/runs/release-pipeline/stado/")
                && job.output_uri.contains("/deliveries/")
                && job.output_uri.ends_with("/output")
        });
        // A host can accumulate deliveries while it is under pressure.
        // The newest submission is the current operator intent; replaying
        // them FIFO briefly downgrades the installed agent before climbing
        // through every superseded coordinate.
        let matched_deliveries = queued.len();
        queued.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.job_id.cmp(&a.job_id))
        });
        queued.truncate(1);
        agent_diag.insert(
            "disk_pressure_superseded_deliveries".into(),
            Value::from(matched_deliveries.saturating_sub(queued.len()) as i64),
        );
        agent_diag.insert(
            "disk_pressure_recovery_jobs".into(),
            Value::from(queued.len() as i64),
        );
        if queued.is_empty() {
            log_fn(&format!(
                "loop: disk-pressure-active: {} bytes free is under the {} byte low \
                 watermark; ordinary work remains blocked and no signed Stado release \
                 delivery is assigned to this host",
                current_free_bytes.unwrap_or_default(),
                disk_low_bytes.unwrap_or_default()
            ));
            tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_S)).await;
            return Ok(Step::Done);
        }
        log_fn(&format!(
            "loop: disk-pressure-active: {} bytes free is under the {} byte low watermark; \
             admitting {} signed Stado release delivery and no ordinary work",
            current_free_bytes.unwrap_or_default(),
            disk_low_bytes.unwrap_or_default(),
            queued.len()
        ));
    }
    Ok(Step::Go(queued))
}
