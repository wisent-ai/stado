//! The claim itself: the last eligibility re-check against the fresh queue
//! record, the janitor's workload lock, board placement, and the bookkeeping
//! one started slot costs the rest of this scan.

use chrono::Utc;
use serde_json::{Map, Value};

use crate::models::{activation_extraction_must_share_gpu, isoformat_utc, Job};
use crate::providers::local::disk_cleanup;
use crate::providers::local::helpers;
use crate::providers::local::slots::{
    job_system_packages_eligible, start_slot, ActiveSlot, StartSlotError,
};
use crate::queue::JobStorage;

/// Claim and start one candidate. `true` means this scan is finished — the
/// host is saturated, or the janitor holds the lock a workload needs.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn start_candidate(
    store: &JobStorage,
    job: &Job,
    hostname: &str,
    consumer_id: &str,
    kind: &str,
    gpu_type: &str,
    total_vram_gb: i64,
    pinned_only: bool,
    vram_buffer_gb: i64,
    need: i64,
    requested_cpu_cores: i64,
    requested_memory_gb: f64,
    is_raw_share: bool,
    raw_reserve: f64,
    available_cpu_cores: &mut i64,
    available_ram_gb: &mut f64,
    raw_reserved: &mut f64,
    card_budget: &mut [(String, i64)],
    free_vram_gb: &mut i64,
    slots: &mut Vec<ActiveSlot>,
    agent_diag: &mut Map<String, Value>,
    diag_eligibility_rejected: &mut i64,
    diag_eligible: &mut i64,
    diag_claim_errors: &mut i64,
    started: &mut i64,
    log_fn: &mut dyn FnMut(&str),
) -> anyhow::Result<bool> {
    if let Some(reason) = helpers::eligibility_refusal(
        job,
        gpu_type,
        total_vram_gb,
        kind,
        consumer_id,
        slots.len(),
        pinned_only,
    ) {
        *diag_eligibility_rejected += 1;
        // The count alone read `eligibility_rejected=72,
        // eligible_count=0` on the always-on mac for seven days and
        // named none of the nine rules that produce it, so the number
        // could not distinguish a wrong pin from a wrong platform and
        // an operator holding it had to reconstruct the answer from the
        // host's process table. The newest refusal carries its rule and
        // the job it judged.
        agent_diag.insert(
            "last_eligibility_reject_job_id".into(),
            Value::from(job.job_id.clone()),
        );
        agent_diag.insert("last_eligibility_reject_reason".into(), Value::from(reason));
        return Ok(false);
    }
    // The listing is only a snapshot. Reapply every admission
    // predicate to the fresh queue record before taking the workload
    // lock or performing any claim/start side effect.
    if !job_system_packages_eligible(job, kind) {
        *diag_eligibility_rejected += 1;
        return Ok(false);
    }
    *diag_eligible += 1;
    // The disk-cleanup workload lock (Python
    // `acquire_workload_lock`): a shared hold on the janitor's
    // lock file for as long as the workload owns its slot.
    let workload_lock = match disk_cleanup::acquire_workload_lock() {
        Ok(lock) => lock,
        Err(exc) => {
            log_fn(&format!(
                "disk cleanup workload lock unavailable: {}",
                exc.code
            ));
            agent_diag.insert(
                "disk_cleanup_admission".into(),
                Value::from(format!("{}:{}", disk_cleanup::CLEANUP_LOCK_ERROR, exc.code)),
            );
            return Ok(true);
        }
    };
    let Some(workload_lock) = workload_lock else {
        agent_diag.insert(
            "disk_cleanup_admission".into(),
            Value::from(disk_cleanup::CLEANUP_IN_PROGRESS),
        );
        return Ok(true);
    };
    // Which board. A job that deliberately shares the GPU joins the
    // board its co-tenant is already on -- sharing means one card, not
    // "any card" -- and everything else takes the emptiest board that
    // can hold it. One card, or none, means nothing to choose and the
    // job keeps the driver's default.
    let shared_uuid = if is_raw_share {
        slots
            .iter()
            .find(|s| activation_extraction_must_share_gpu(&s.slot.job.command))
            .and_then(|s| s.gpu_uuid.clone())
    } else {
        None
    };
    let placement = if card_budget.len() < 2 {
        None
    } else if let Some(uuid) = shared_uuid {
        Some(uuid)
    } else {
        card_budget
            .iter()
            .filter(|(_, free)| *free >= need)
            .max_by_key(|(_, free)| *free)
            .map(|(uuid, _)| uuid.clone())
    };
    let new_slot = match start_slot(
        store,
        job.clone(),
        hostname,
        log_fn,
        kind,
        placement.as_deref(),
    )
    .await
    {
        Ok(slot) => slot,
        Err(StartSlotError::Claim(exc)) => {
            // One job's claim is that job's problem. Returning here
            // ends the tick, and `cli::agent` restarts the whole loop:
            // on charless-mac-mini a single queued job whose durable
            // transition record could not be verified killed the loop
            // every few seconds for hours, so the nine other queued
            // jobs were never reached, the census keys never survived
            // a publish, and every gate read the host as healthy. The
            // same doctrine `cli::doctor` states for probes holds here:
            // one failure names itself and the scan continues.
            disk_cleanup::release_workload_lock(workload_lock, log_fn);
            log_fn(&format!(
                "claim refused for {}: {}; skipping this job and continuing the scan",
                job.job_id, exc
            ));
            *diag_claim_errors += 1;
            agent_diag.insert(
                "last_claim_error_job".into(),
                Value::from(job.job_id.clone()),
            );
            agent_diag.insert("last_claim_error".into(), Value::from(exc.to_string()));
            agent_diag.insert(
                "last_claim_error_at".into(),
                Value::from(isoformat_utc(Utc::now())),
            );
            return Ok(false);
        }
        Err(StartSlotError::Other(exc)) => {
            disk_cleanup::release_workload_lock(workload_lock, log_fn);
            return Err(exc.into());
        }
    };
    let Some(mut new_slot) = new_slot else {
        // Admission failed before spawn; do not retain a workload lock.
        disk_cleanup::release_workload_lock(workload_lock, log_fn);
        return Ok(false);
    };
    new_slot.disk_cleanup_lock = Some(workload_lock);
    let exclusive_started = helpers::slot_is_exclusive(&new_slot.slot);
    slots.push(new_slot);
    *available_cpu_cores = available_cpu_cores.saturating_sub(requested_cpu_cores);
    *available_ram_gb = (*available_ram_gb - requested_memory_gb).max(0.0);
    *free_vram_gb -= need;
    if let Some(uuid) = &placement {
        if let Some(entry) = card_budget.iter_mut().find(|(id, _)| id == uuid) {
            entry.1 = (entry.1 - need).max(0);
        }
    }
    if is_raw_share {
        *raw_reserved += raw_reserve;
    }
    *started += 1;
    agent_diag.insert(
        "last_started_job_id".into(),
        Value::from(job.job_id.clone()),
    );
    agent_diag.insert(
        "last_started_at".into(),
        Value::from(isoformat_utc(Utc::now())),
    );
    if exclusive_started
        || *available_cpu_cores == 0
        || *available_ram_gb < 1.0
        || *free_vram_gb <= vram_buffer_gb
    {
        return Ok(true);
    }
    Ok(false)
}
