//! Cooperative priority yield.

use std::time::{Duration, Instant};

use crate::config::estimate_gpu_memory;
use crate::providers::local::helpers;
use crate::providers::local::slots::{
    job_system_packages_eligible, request_yield, ActiveSlot, DEFAULT_MAX_YIELDS,
};
use crate::queue::{JobStorage, StorageError};
use crate::sizing::Sizing;

use super::super::{MIN_RUNTIME_BEFORE_YIELD_S, QUEUE_SCAN_BUDGET, YIELD_CANDIDATE_WINDOW};

/// The yield-relevant facts of one running slot, extracted so the eviction
/// choice is a pure function (Python reads these off the slot dict + Job).
#[derive(Debug, Clone)]
pub struct YieldSlotInfo {
    pub job_id: String,
    pub priority: i64,
    pub yieldable: bool,
    /// `_slot_is_exclusive(slot)` — exclusive slots are never evicted.
    pub exclusive: bool,
    pub yield_count: i64,
    pub max_yields_before_protected: i64,
    pub started_mono: Instant,
    /// `_slot_vram(slot)` — best known VRAM footprint.
    pub vram_gb: i64,
}

/// Choose which slots to yield so `need` GB fits. Pure: the eviction half
/// of Python `_maybe_yield_for_priority`.
///
/// Evictable = yieldable, non-exclusive, strictly lower priority than the
/// target, not yet yield-protected, and past the anti-thrash runtime floor.
/// Evict lowest-priority first; among equal priority, free the largest slot
/// first so we yield as few jobs as possible. Returns empty when even
/// yielding every candidate won't fit — don't waste a yield.
pub fn choose_yield_slots(
    slots: &[YieldSlotInfo],
    target_prio: i64,
    need: i64,
    free_vram_gb: i64,
    now: Instant,
) -> Vec<usize> {
    let mut evictable: Vec<usize> = (0..slots.len())
        .filter(|&i| {
            let s = &slots[i];
            // Python `int(getattr(job, "max_yields_before_protected", 5) or 5)`:
            // a stored 0 falls back to 5.
            let max_yields = if s.max_yields_before_protected != 0 {
                s.max_yields_before_protected
            } else {
                DEFAULT_MAX_YIELDS
            };
            s.yieldable
                && !s.exclusive
                && s.priority < target_prio
                && s.yield_count < max_yields
                && now.saturating_duration_since(s.started_mono)
                    >= Duration::from_secs(MIN_RUNTIME_BEFORE_YIELD_S)
        })
        .collect();
    if evictable.is_empty() {
        return Vec::new();
    }
    evictable.sort_by_key(|&i| (slots[i].priority, -slots[i].vram_gb));
    let mut freed = 0i64;
    let mut chosen = Vec::new();
    for i in evictable {
        chosen.push(i);
        freed += slots[i].vram_gb;
        if free_vram_gb + freed >= need {
            break;
        }
    }
    if free_vram_gb + freed < need {
        return Vec::new();
    }
    chosen
}

/// If a strictly-higher-priority eligible queued job can't fit in the
/// current free VRAM, cooperatively yield just enough lower-priority
/// yieldable slots to make room. Returns the number of slots yielded
/// (removed from `slots`); 0 means no action.
/// Python `_maybe_yield_for_priority`.
///
/// Inert by construction: returns immediately unless a yieldable job is
/// actually running, so existing (non-yieldable) prod workloads never enter
/// the queue scan or any eviction logic.
#[allow(clippy::too_many_arguments)]
pub async fn maybe_yield_for_priority(
    store: &JobStorage,
    sizing: &Sizing,
    slots: &mut Vec<ActiveSlot>,
    gpu_type: &str,
    total_vram_gb: i64,
    free_vram_gb: i64,
    kind: &str,
    consumer_id: &str,
    log_fn: &mut dyn FnMut(&str),
) -> Result<usize, StorageError> {
    if !slots.iter().any(|s| s.slot.job.yieldable) {
        return Ok(0);
    }
    // Highest-priority queued job that needs MORE than current free VRAM but
    // could fit on the full GPU, and is eligible for THIS agent.
    // The window counts jobs THIS agent could admit. Counting merely fitting
    // jobs let a page of another worker's or another platform's work fill it
    // and hid the higher-priority job this host is meant to make room for.
    let mut candidates = store
        .list_claimable_jobs(
            "queue",
            &crate::queue::listing::JobScan {
                want: YIELD_CANDIDATE_WINDOW,
                scan_budget: QUEUE_SCAN_BUDGET,
                max_gpu_mem_gb: total_vram_gb,
                eligible: &|job| {
                    helpers::job_eligible(
                        job,
                        gpu_type,
                        total_vram_gb,
                        kind,
                        consumer_id,
                        slots.len(),
                        false,
                    ) && job_system_packages_eligible(job, kind)
                },
                // Preempting a running job is a priority-fidelity decision,
                // not a reachability one: it must be taken against the head
                // of the index. The rotating cursor is shared with the claim
                // loops below, so inheriting it would answer "the most
                // important job in some rotated slice", and this host would
                // yield to the wrong job — or fail to yield at all while the
                // job it should make room for sat before the slice.
                from_head: true,
            },
        )
        .await?;
    candidates.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| a.created_at.cmp(&b.created_at))
    });
    let mut target: Option<(crate::models::Job, i64)> = None;
    for candidate in &candidates {
        let Some(j) = store.read_job("queue", &candidate.job_id).await? else {
            continue;
        };
        if !helpers::job_eligible(
            &j,
            gpu_type,
            total_vram_gb,
            kind,
            consumer_id,
            slots.len(),
            false,
        ) || !job_system_packages_eligible(&j, kind)
        {
            continue;
        }
        let need_j = j
            .gpu_mem_gb
            .max(estimate_gpu_memory(&j.command, sizing, store).await?);
        if need_j <= free_vram_gb {
            continue; // already fits — not a VRAM-eviction case
        }
        target = Some((j, need_j));
        break;
    }
    let Some((target, need)) = target else {
        return Ok(0);
    };
    let target_prio = target.priority;

    let now = Instant::now();
    let mut infos = Vec::with_capacity(slots.len());
    for s in slots.iter() {
        infos.push(YieldSlotInfo {
            job_id: s.slot.job.job_id.clone(),
            priority: s.slot.job.priority,
            yieldable: s.slot.job.yieldable,
            exclusive: helpers::slot_is_exclusive(&s.slot),
            yield_count: s.slot.job.yield_count,
            max_yields_before_protected: s.slot.job.max_yields_before_protected,
            started_mono: s.started_mono,
            vram_gb: helpers::slot_vram(&s.slot, sizing, store).await?,
        });
    }
    let chosen = choose_yield_slots(&infos, target_prio, need, free_vram_gb, now);
    if chosen.is_empty() {
        return Ok(0);
    }
    let freed: i64 = chosen.iter().map(|&i| infos[i].vram_gb).sum();
    let mut n = 0usize;
    // Remove in descending index order so earlier removals don't shift the
    // indices of later ones (Python `slots.remove(s)` on identity).
    for &idx in chosen.iter().rev() {
        let s = slots.remove(idx);
        let jid = s.slot.job.job_id.clone();
        match request_yield(s, store, log_fn).await {
            Ok(true) => n += 1,
            Ok(false) => {}
            Err(exc) => log_fn(&format!("yield: request_yield raised for {jid}: {exc}")),
        }
    }
    if n > 0 {
        log_fn(&format!(
            "yield: freed ~{freed}G via {n} slot(s) for higher-priority {} (need={need}G prio={target_prio})",
            target.job_id
        ));
    }
    Ok(n)
}
