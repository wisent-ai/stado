//! The tick's own broadcast, the maintenance-mode read that must agree with
//! it, and the cooperative yield that makes room before anything is claimed.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use crate::primitives::constants;
use crate::providers::local::agent::capacity::snapshot::{measured_capacity, publish_branch};
use crate::providers::local::agent::{maybe_yield_for_priority, Step, POLL_INTERVAL_S};
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
    claim_store_deadline: Instant,
    available_accelerators: BTreeMap<String, i64>,
    slots: &mut Vec<ActiveSlot>,
    agent_diag: &mut Map<String, Value>,
    last_cap: &mut Option<CapacitySnapshot>,
    log_fn: &mut dyn FnMut(&str),
) -> anyhow::Result<Step<()>> {
    let claim_budget_left = || claim_store_deadline.saturating_duration_since(Instant::now());
    // Maintenance mode is read before the publication: the row must report
    // the same admission decision the loop will enforce below. Jobs already
    // running were advanced earlier and continue normally.
    //
    // Re-read every iteration, never cached: `stado queue resume` has to
    // reach a running agent without restarting it. The read shares the
    // tick's store budget; a timeout publishes an explicit refusal rather
    // than leaving the previous accepting decision live.
    let Ok(queue_control) = tokio::time::timeout(claim_budget_left(), control::read(store)).await
    else {
        let snapshot = measured_capacity(
            slots,
            false,
            Some("queue_control_unavailable"),
            available_accelerators.clone(),
            free_vram_gb,
            total_vram_gb,
            agent_diag.clone(),
        );
        let _ = publish_branch(
            store,
            consumer_id,
            kind,
            "queue-control-unavailable",
            &snapshot,
            log_fn,
        )
        .await;
        *last_cap = Some(snapshot);
        log_fn(&format!(
            "loop: queue-control read exhausted this tick's {}s store budget; claiming nothing this tick",
            constants::AGENT_CLAIM_STORE_BUDGET_S
        ));
        tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_S)).await;
        return Ok(Step::Done);
    };
    let queue_control = queue_control?;
    agent_diag.insert("queue_paused".into(), Value::from(queue_control.paused));
    // Which build is answering for this host. The broadcast carried a
    // capacity verdict, a claim-loop census and a disk report and never the
    // version that produced them, so after `host release` installed 0.9.5
    // and `service converge` reported `installed 0.9.5, in-sync`, there was
    // no way to tell whether the process still refusing every pinned job
    // was the new binary or one of the older ones the same host was running
    // — and the question had to be answered by reading process ages out of
    // `ps`, on a machine whose pid counter had wrapped.
    // The version alone did not finish the job. `0.14.6` named four
    // different trees of this crate on 2026-09-03, and a host publishing
    // `agent_version: "0.14.6"` still left "which build is this" to be
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
    let policy_reason = if queue_control.paused {
        Some("queue_paused")
    } else if pressure_active {
        Some("disk_pressure_active")
    } else {
        None
    };
    let snapshot = measured_capacity(
        slots,
        policy_reason.is_none(),
        policy_reason,
        available_accelerators,
        free_vram_gb,
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
        tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_S)).await;
        return Ok(Step::Done);
    }
    if !accepting_jobs && !pressure_active {
        let reason = last_cap
            .as_ref()
            .and_then(|capacity| capacity.diag.get("admission_reason"))
            .and_then(Value::as_str)
            .unwrap_or("resources_busy");
        log_fn(&format!(
            "admission refused by live resources: {reason}; skipping claims"
        ));
        tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_S)).await;
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
        && maybe_yield_for_priority(
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
            > 0
    {
        // re-loop: recompute free VRAM, then claim the freed room
        return Ok(Step::Done);
    }
    Ok(Step::Go(()))
}
