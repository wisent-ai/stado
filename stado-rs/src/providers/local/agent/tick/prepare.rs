//! What a run sets up once, and what one tick does before it can say anything
//! about capacity: breadcrumbs, this tick's two store budgets, the running
//! slots, the release handoff, and the janitor's last finished pass.

use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use crate::constants;
use crate::providers::local::agent_heartbeat::CapacityHeartbeat;
use crate::providers::local::agent_janitor::{JanitorReports, JanitorTask};
use crate::providers::local::disk_cleanup;
use crate::providers::local::helpers;
use crate::providers::local::slots::{advance_slot, ActiveSlot, SlotOutcome};
use crate::queue::capacity::CapacitySnapshot;
use crate::queue::JobStorage;
use crate::sizing::Sizing;

use super::super::capacity::release::installed_stado_release_mismatch;
use super::super::{agent_log, ReleaseHandoff};

/// Which store this agent just bound to, and whether a coordinate written
/// there means anything to anyone else.
///
/// A capacity broadcast is a claim about the fleet. Written into a store
/// whose reach is `Device` the write does not fail -- it succeeds, and every
/// other host reports the broadcast absent, which is indistinguishable from
/// an agent that never ran. That is what the always-on mac did for an
/// afternoon: its unit was re-declared carrying `STADO_CONFIG`, the host
/// configuration behind that path selects `storage.backend: local`, and from
/// that moment the agent published into a directory on its own disk while the
/// scheduler read a frozen row and 55 jobs pinned to that host waited on a
/// capacity number nobody was writing any more.
///
/// The agent keeps running: it is also the host's disk janitor, and stopping
/// cleanup on a machine under disk pressure trades one outage for another.
/// What it must not do is stay quiet about it. The store is named on every
/// iteration, in the log, which is the one channel that still reaches an
/// operator when the store itself is the thing that is wrong.
pub(super) fn bound_store(log_fn: &mut dyn FnMut(&str)) -> (&'static str, bool) {
    let storage_backend = crate::config::wc_storage_backend();
    let store_reach = crate::capabilities::storage_reach(storage_backend);
    let store_answers_for_fleet = store_reach == Some(crate::capabilities::StorageReach::Fleet);
    log_fn(&format!(
        "init: capacity broadcasts go to the {storage_backend:?} store, which answers for {}",
        match store_reach {
            Some(crate::capabilities::StorageReach::Fleet) => "the fleet",
            Some(crate::capabilities::StorageReach::Device) => "this machine only",
            None => "an unknown scope: this build does not know that backend",
        }
    ));
    (storage_backend, store_answers_for_fleet)
}

/// The janitor owns its own cadence from here. It is still invoked at the
/// tick's poll interval -- the cleanup engine's own lock and policy decide
/// what a pass does -- but off the critical path, so a long pass delays only
/// the next pass and never a capacity broadcast. The task is held in scope for
/// the agent's lifetime: dropping the handle aborts the pass loop, so a release
/// handoff does not leave a janitor behind.
pub(super) fn spawn_janitor() -> (JanitorReports, JanitorTask) {
    let janitor_reports = JanitorReports::new();
    let janitor = janitor_reports.spawn_janitor(
        std::time::Duration::from_secs(crate::constants::POLL_INTERVAL_S),
        |active_jobs| async move {
            // Off the critical path, beside the disk pass, for the same reason:
            // an expired lease is host garbage, and the host is the only thing
            // that always knows it holds one.
            crate::providers::local::scratch_sweep::sweep(&mut |msg: &str| agent_log(msg)).await;
            disk_cleanup::run_cleanup_once(
                active_jobs,
                false,
                disk_cleanup::CleanupWriter::AgentTick,
                &mut |msg: &str| agent_log(msg),
            )
            .await
        },
    );
    (janitor_reports, janitor)
}

/// Advance every running slot, then report `(tick deadline, claim deadline,
/// vast renter active)` for the rest of this tick to spend.
#[allow(clippy::too_many_arguments)]
pub(super) async fn advance_slots(
    store: &JobStorage,
    sizing: &Sizing,
    heartbeat: &CapacityHeartbeat,
    janitor_reports: &JanitorReports,
    storage_backend: &str,
    store_answers_for_fleet: bool,
    last_cap: &Option<CapacitySnapshot>,
    slots: &mut Vec<ActiveSlot>,
    agent_diag: &mut Map<String, Value>,
    disk_low_bytes: &mut Option<i64>,
    log_fn: &mut dyn FnMut(&str),
) -> anyhow::Result<(Instant, Instant, bool)> {
    // Phase breadcrumbs for the 40GB a2-highgpu-1g first-iter hang.
    log_fn("loop: iter-start");
    heartbeat.record_tick_start();
    // One place, not four: whatever branch of the previous iteration
    // published, `last_cap` holds it, so the heartbeat repeats the tick's
    // own most recent measurement and never a figure of its own.
    if let Some(cap) = last_cap {
        heartbeat.record_published(cap.clone());
    }
    // Every broadcast says which store wrote it. A reader holding a frozen
    // row could not tell a stopped agent from a running one publishing
    // somewhere else, and that is the question that took an afternoon.
    agent_diag.insert("storage_backend".into(), Value::from(storage_backend));
    agent_diag.insert(
        "storage_answers_for_fleet".into(),
        Value::from(store_answers_for_fleet),
    );
    if !store_answers_for_fleet {
        log_fn(&format!(
            "loop: this agent's {storage_backend:?} store does not answer for the fleet, so \
             every capacity broadcast below is invisible to the scheduler and to every other \
             host; the queue it reads is not the fleet queue"
        ));
    }
    // ONE budget for everything this tick reads out of the store, shared
    // across the reads rather than handed out per read.
    //
    // Per-read budgets were the first shape of this and they were wrong in
    // a way the host said out loud: with 20 s each, the claimable-job
    // listing -- much the heaviest read, and the only one claiming depends
    // on -- timed out on every tick against this store, so the host stayed
    // fresh and still claimed nothing. Freshness bought by never claiming
    // is not the fix. A shared deadline spends the budget where the tick
    // actually needs it: the small documents normally answer in under a
    // second and leave nearly the whole allowance to the listing.
    //
    // Two deadlines, because the two halves of a tick answer different
    // questions. Everything the tick reads BEFORE its own publication
    // shares [`constants::AGENT_TICK_STORE_BUDGET_S`]: those reads only
    // refine what the broadcast says, and none of them is worth delaying
    // it. Everything the ADMISSION half reads shares the larger
    // [`constants::AGENT_CLAIM_STORE_BUDGET_S`], because asking a
    // saturated store for work legitimately takes longer than a heartbeat
    // interval and the heartbeat task keeps publishing while it does.
    // Every read below degrades to "keep what we last knew" or "claim
    // nothing this tick"; nothing mutating is inside either deadline.
    let tick_store_deadline =
        Instant::now() + Duration::from_secs(constants::AGENT_TICK_STORE_BUDGET_S);
    let store_budget_left = || tick_store_deadline.saturating_duration_since(Instant::now());
    let claim_store_deadline =
        Instant::now() + Duration::from_secs(constants::AGENT_CLAIM_STORE_BUDGET_S);
    match tokio::time::timeout(
        store_budget_left(),
        crate::config::refresh_model_policy(store),
    )
    .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(exc)) => log_fn(&format!(
            "model policy refresh failed; retaining last good policy: {exc}"
        )),
        Err(_) => log_fn(&format!(
            "model policy refresh exhausted this tick's {}s store budget; retaining last good \
             policy so this tick still publishes capacity and claims",
            constants::AGENT_TICK_STORE_BUDGET_S
        )),
    }
    // DEVIATION: the wisent upload_worker sweep is not ported (the
    // wisent Python package owns it); the fleet-flush subprocess path
    // below covers the same pending pool.
    let vast_active = if crate::config::wc_providers()
        .iter()
        .any(|provider| provider == crate::capabilities::ProviderId::Vast.as_str())
        && !crate::config::wc_disabled_providers()
            .iter()
            .any(|provider| provider == crate::capabilities::ProviderId::Vast.as_str())
    {
        helpers::vast_has_renter().await?
    } else {
        false
    };
    let mut survivors: Vec<ActiveSlot> = Vec::with_capacity(slots.len());
    for slot in slots.drain(..) {
        match advance_slot(slot, store, sizing, vast_active, log_fn).await? {
            SlotOutcome::Running(slot) => survivors.push(slot),
            SlotOutcome::Done => {}
        }
    }
    *slots = survivors;
    if slots.is_empty() {
        if let Some(installed) = installed_stado_release_mismatch(log_fn) {
            let detail = format!(
                "installed Stado {installed} supersedes running {}; exiting after all slots \
                 finished so the declared supervisor starts the installed release",
                env!("CARGO_PKG_VERSION")
            );
            log_fn(&format!("loop: release-handoff: {detail}"));
            // Linux services use Restart=on-failure; launchd KeepAlive also
            // recreates this process. The command wrapper must propagate
            // this typed error instead of treating it as a retryable loop
            // failure inside the same process.
            return Err(ReleaseHandoff(detail).into());
        }
    }
    // The janitor's bounded cleanup pass runs on its own task
    // ([`crate::providers::local::agent_janitor`]); this tick only reads
    // passes that have already finished. Awaiting the pass here is what made a
    // release builder invisible: a 13.6-minute `healthy_noop` pass held the
    // capacity publication far past the 180s staleness cutoff, so
    // `release submit` refused a healthy, correctly declared builder.
    // Publication must happen at the heartbeat interval whatever the
    // janitor is doing, so nothing on this line may ever wait for it.
    janitor_reports.set_active_jobs(slots.len() as i64);
    if let Some(cleanup_report) = janitor_reports.latest() {
        if let Some(reported_low) = disk_cleanup::validated_report_low_bytes(&cleanup_report) {
            *disk_low_bytes = Some(reported_low);
        }
        agent_diag.insert("disk_cleanup".into(), cleanup_report);
    } else {
        // No pass has completed yet. Say so rather than leaving the key
        // absent, which reads identically to a wedged janitor.
        agent_diag.insert(
            "disk_cleanup".into(),
            serde_json::json!({"outcome": "no_pass_completed_yet"}),
        );
    }
    Ok((tick_store_deadline, claim_store_deadline, vast_active))
}
