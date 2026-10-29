//! The tick's own broadcast, the maintenance-mode read that must agree with
//! it, and the cooperative yield that makes room before anything is claimed.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::providers::local::agent::capacity::snapshot::{measured_capacity, publish_branch};
use crate::providers::local::agent::{maybe_yield_for_priority, Step};
use crate::providers::local::self_terminate;
use crate::providers::local::slots::ActiveSlot;
use crate::queue::capacity::CapacitySnapshot;
use crate::queue::control;
use crate::queue::JobStorage;
use crate::sizing::Sizing;

/// Publish what this tick measured and decide whether it claims at all.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn publish_and_admit(
    store: &JobStorage,
    sizing: &Sizing,
    consumer_id: &str,
    kind: &str,
    gpu_type: &str,
    total_vram_gb: i64,
    free_vram_gb: i64,
    idle_shutdown: bool,
    pressure_active: bool,
    available_accelerators: BTreeMap<String, i64>,
    slots: &mut Vec<ActiveSlot>,
    agent_diag: &mut Map<String, Value>,
    last_cap: &mut Option<CapacitySnapshot>,
    log_fn: &mut dyn FnMut(&str),
) -> anyhow::Result<Step<()>> {
    // Maintenance mode is read before the publication: the row must report
    // the same admission decision the loop will enforce below. Jobs already
    // running were advanced earlier and continue normally.
    //
    // Re-read every iteration, never cached: `stado queue resume` has to
    // reach a running agent without restarting it. A read that fails ends the
    // tick with the store's error.
    let queue_control = control::read(store).await?;
    agent_diag.insert("queue_paused".into(), Value::from(queue_control.paused));
    // Which build is answering for this host. The broadcast carried a
    // capacity verdict, a claim-loop census and a disk report and never the
    // version that produced them, so after `host release` installed 0.9.5
    // and `release version show` reported `installed 0.9.5, in-sync`, there was
    // no way to tell whether the process still refusing every pinned job
    // was the new binary or one of the older ones the same host was running
    // — and the question had to be answered by reading process ages out of
    // `ps`, on a machine whose pid counter had wrapped.
    // The version alone did not finish the job. One version string can name
    // several different trees of this crate, and a host publishing only
    // `agent_version` still leaves "which build is this" to be
    // answered by reading symbols out of the binary. The identity carries
    // the revision, and the revision is published beside it so a reader
    // does not have to parse the sentence to get at it.
    agent_diag.insert(
        "agent_version".into(),
        Value::from(crate::binary::build_identity::BUILD_IDENTITY),
    );
    agent_diag.insert(
        "agent_source_revision".into(),
        Value::from(crate::binary::build_identity::SOURCE_REVISION),
    );
    // Whether a claim could take the workload lock right now. The claim path
    // asks the same question and answers `cleanup_in_progress` without a
    // claim, so a publication saying `accepting_jobs: true` meanwhile is a
    // host promising work it cannot start: while a janitor thread holds the
    // lock inside a consent-gated `openat`, `host gates` reads `claiming:
    // yes` and a required release delivery sits queued against it. The
    // probe takes the
    // shared lock and releases it at once, which is what the claim would do,
    // and what it saw is published under the same key the claim path writes.
    let probe = crate::providers::local::space_reclaim::acquire_workload_lock("admission-probe");
    let lock_reason = match probe {
        Ok(Some(probe)) => {
            crate::providers::local::space_reclaim::release_workload_lock(probe, log_fn);
            agent_diag.remove("space_reclaim_admission");
            None
        }
        Ok(None) => {
            agent_diag.insert(
                "space_reclaim_admission".into(),
                Value::from(crate::providers::local::space_reclaim::CLEANUP_IN_PROGRESS),
            );
            Some(crate::providers::local::space_reclaim::CLEANUP_IN_PROGRESS)
        }
        Err(exc) => {
            // The claim path stops for this too, so the publication says so.
            log_fn(&format!(
                "space reclaim workload lock unavailable: {}",
                exc.code
            ));
            agent_diag.insert(
                "space_reclaim_admission".into(),
                Value::from(format!(
                    "{}:{}",
                    crate::providers::local::space_reclaim::CLEANUP_LOCK_ERROR,
                    exc.code
                )),
            );
            Some(crate::providers::local::space_reclaim::CLEANUP_LOCK_ERROR)
        }
    };
    // A host on battery claims nothing: work drains it until it sleeps and
    // drops every job. While it holds work it is kept awake on mains.
    let power = crate::providers::local::host_power::read();
    power.publish(agent_diag);
    crate::providers::local::host_power::awake::hold_while(!slots.is_empty(), log_fn);
    // A drain asked of this worker ([`crate::providers::local::agent::drain`])
    // admits nothing more, and the publication says so with the same reason.
    let drain = crate::providers::local::agent::drain::requested();
    let policy_reason = if queue_control.paused {
        Some("queue_paused")
    } else if drain.is_some() {
        Some("drain_requested")
    } else if pressure_active {
        Some("disk_pressure_active")
    } else if power.on_mains == Some(false) {
        Some(crate::providers::local::host_power::ON_BATTERY)
    } else {
        lock_reason
    };
    let snapshot = measured_capacity(
        slots,
        policy_reason.is_none(),
        policy_reason,
        available_accelerators,
        Some(free_vram_gb),
        total_vram_gb,
        agent_diag.clone(),
    );
    let accepting_jobs = snapshot.accepting_jobs;
    publish_branch(
        store,
        consumer_id,
        kind,
        "admission-measured",
        &snapshot,
        log_fn,
    )
    .await?;
    *last_cap = Some(snapshot);
    if queue_control.paused {
        agent_diag.insert(
            "queue_pause_reason".into(),
            Value::from(queue_control.pause_summary()),
        );
        log_fn(&format!(
            "Queue paused ({}); claiming nothing",
            queue_control.pause_summary()
        ));
        // An ephemeral cloud agent with no running jobs is idle while
        // paused. Exit so the capacity heartbeat stops; the owning provider
        // adapter reaps the machine without replacing it.
        if idle_shutdown && slots.is_empty() {
            log_fn("idle_shutdown: no running jobs + queue paused; exiting");
            self_terminate(kind, log_fn).await;
            return Ok(Step::Stop);
        }
        return Ok(Step::Done);
    }
    if let Some(reason) = drain {
        agent_diag.insert("drain_reason".into(), Value::from(reason.clone()));
        // The slots already running were advanced before this gate; the
        // loop ends once none is left.
        if slots.is_empty() {
            log_fn(&format!("drain: {reason}; no job runs here, exiting"));
            return Ok(Step::Stop);
        }
        log_fn(&format!(
            "drain: {reason}; claiming nothing, {} running job(s) finish first",
            slots.len()
        ));
        return Ok(Step::Done);
    }
    if !accepting_jobs && !pressure_active {
        match last_cap
            .as_ref()
            .and_then(|capacity| capacity.diag.get("admission_reason"))
            .and_then(Value::as_str)
        {
            Some(reason) => log_fn(&format!(
                "admission refused by live resources: {reason}; skipping claims"
            )),
            None => log_fn(
                "admission refused by live resources, whose capacity reading names no \
                 admission_reason; skipping claims",
            ),
        }
        return Ok(Step::Done);
    }
    // Disk pressure is enforced after the assigned queue is read. One exact
    // signed release-delivery command remains admissible so Stado can repair
    // the agent binary that owns this gate; every ordinary workload remains
    // blocked below the watermark.

    // Cooperative yield: if a higher-priority queued job can't fit, evict
    // just enough lower-priority yieldable slots to make room. Runs BEFORE
    // the full-GPU early-return below because that is exactly when it's
    // needed. Inert (single any() over slots) unless a yieldable job runs.
    if !pressure_active
        && !maybe_yield_for_priority(
            store,
            sizing,
            slots,
            gpu_type,
            total_vram_gb,
            free_vram_gb,
            kind,
            consumer_id,
            log_fn,
        )
        .await?
        .is_empty()
    {
        // re-loop: recompute free VRAM, then claim the freed room
        return Ok(Step::Done);
    }
    Ok(Step::Go(()))
}
