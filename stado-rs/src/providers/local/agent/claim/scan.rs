//! One claim scan: the budgets it opens with, the candidates it walks, and
//! the census it leaves behind in the diagnostics.

use chrono::Utc;
use serde_json::{Map, Value};

use crate::models::{isoformat_utc, Job};
use crate::providers::local::helpers;
use crate::providers::local::slots::{ActiveSlot, CLAIM_DECLINED_KEY};
use crate::queue::capacity::CapacitySnapshot;
use crate::queue::JobStorage;
use crate::sizing::Sizing;

use super::fit::candidate_fit;
use super::start::start_candidate;

/// What one claim scan did, job by job: each list holds the ids of the jobs
/// that reached that outcome, so a published count is the length of a list
/// that names its jobs, never a counter nothing explains.
#[derive(Default)]
pub(crate) struct Census {
    pub cpu_rejected: Vec<String>,
    pub ram_rejected: Vec<String>,
    pub vram_rejected: Vec<String>,
    pub eligibility_rejected: Vec<String>,
    pub eligible: Vec<String>,
    pub claim_errors: Vec<String>,
    pub declined: Vec<Value>,
    pub started: Vec<String>,
}

impl Census {
    /// Publish the scan into the diagnostics: the count of each outcome, and
    /// the declined jobs with their reasons.
    fn publish(self, queued: &[Job], agent_diag: &mut Map<String, Value>) {
        let counts = [
            ("vram_rejected", self.vram_rejected.len()),
            ("cpu_rejected", self.cpu_rejected.len()),
            ("ram_rejected", self.ram_rejected.len()),
            ("eligibility_rejected", self.eligibility_rejected.len()),
            ("eligible_count", self.eligible.len()),
            ("claimed_this_loop", self.started.len()),
            ("claim_errors", self.claim_errors.len()),
            ("queue_scanned", queued.len()),
        ];
        for (key, count) in counts {
            agent_diag.insert(key.into(), Value::from(count));
        }
        agent_diag.insert(CLAIM_DECLINED_KEY.into(), Value::Array(self.declined));
        agent_diag.insert(
            "last_claim_attempt_at".into(),
            Value::from(isoformat_utc(Utc::now())),
        );
    }
}

/// Walk what this tick may claim, start what still fits, and leave the census
/// of the scan in the diagnostics. Reports how many slots it started.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn claim_scan(
    store: &JobStorage,
    sizing: &Sizing,
    hostname: &str,
    consumer_id: &str,
    kind: &str,
    gpu_type: &str,
    total_vram_gb: i64,
    pinned_only: bool,
    queued: &[Job],
    cards: &[helpers::GpuCard],
    last_cap: &Option<CapacitySnapshot>,
    slots: &mut Vec<ActiveSlot>,
    free_vram_gb: &mut i64,
    agent_diag: &mut Map<String, Value>,
    log_fn: &mut dyn FnMut(&str),
) -> anyhow::Result<usize> {
    let mut census = Census::default();
    // These keys describe one completed scan. The previous scan was
    // already published before this point; carrying its last error into a
    // later clean scan would turn a historical refusal into current state.
    for key in [
        "last_claim_error_job",
        "last_claim_error",
        "last_claim_error_at",
        "claim_budget_unread",
    ] {
        agent_diag.remove(key);
    }
    // The CPU and RAM this agent itself measured and broadcast with its
    // capacity. Before it broadcast one, or when the broadcast states no
    // measured CPU or no free RAM, there is no budget to claim against: the
    // scan claims nothing and says why, instead of claiming against an
    // invented empty budget.
    let budget = last_cap
        .as_ref()
        .and_then(|capacity| Some((capacity.available_cpu_cores?, capacity.free_ram_gb?)));
    let Some((mut available_cpu_cores, mut available_ram_gb)) = budget else {
        agent_diag.insert(
            "claim_budget_unread".into(),
            Value::from(
                "this agent has broadcast no capacity stating its free CPU and RAM yet, so \
                 nothing is claimed",
            ),
        );
        let started = census.started.len();
        census.publish(queued, agent_diag);
        return Ok(started);
    };
    // Per-card budget for this tick, emptiest first. The driver's frees
    // already include every allocation that exists; the subtraction below
    // covers the window in which a job this tick admitted has not allocated
    // yet, which is exactly when a second claim would otherwise be sized
    // against memory the first one is about to take.
    let mut card_budget: Vec<(String, i64)> = cards
        .iter()
        .map(|card| (card.uuid.clone(), card.free_vram_gb))
        .collect();
    for job in queued.iter() {
        // A queued job this agent is already running was put back after its
        // lease lapsed under a live process. Its slot takes it back on its own
        // tick (`slots::lifecycle::tick`); it is never started a second time.
        if slots.iter().any(|slot| slot.slot.job.job_id == job.job_id) {
            continue;
        }
        let cmd = job.command.clone();
        let Some((need, requested_cpu_cores, requested_memory_gb)) = candidate_fit(
            store,
            sizing,
            job,
            &cmd,
            total_vram_gb,
            *free_vram_gb,
            available_cpu_cores,
            available_ram_gb,
            slots,
            agent_diag,
            &mut census,
        )
        .await?
        else {
            continue;
        };
        if start_candidate(
            store,
            job,
            hostname,
            consumer_id,
            kind,
            gpu_type,
            total_vram_gb,
            pinned_only,
            need,
            requested_cpu_cores,
            requested_memory_gb,
            &mut available_cpu_cores,
            &mut available_ram_gb,
            &mut card_budget,
            free_vram_gb,
            slots,
            agent_diag,
            &mut census,
            log_fn,
        )
        .await?
        {
            break;
        }
    }
    let started = census.started.len();
    census.publish(queued, agent_diag);
    Ok(started)
}
