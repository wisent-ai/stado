//! The canonical registry target, the disk watermark it declares, and the two
//! things a tick does once both are known: republish, and flush staging.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use crate::primitives::constants;
use crate::providers::local::disk_cleanup;
use crate::providers::local::disk::fleet_flush::spawn_fleet_flush;
use crate::providers::local::slots::ActiveSlot;
use crate::queue::capacity::CapacitySnapshot;
use crate::queue::JobStorage;
use crate::targets::ComputeTarget;

use super::super::capacity::snapshot::{measured_capacity, publish_branch};
use super::super::{lookup_self_auto, Step, POLL_INTERVAL_S};

/// What one tick learns about the disk policy it admits against: the
/// canonical registry target that declared it, the free bytes measured under
/// `$HOME`, and whether this host is below its low watermark.
pub(super) type DiskPolicy = (Option<ComputeTarget>, Option<i64>, bool);

/// Read the disk policy this tick admits against, and report
/// `(registry target, free bytes, pressure active)`.
#[allow(clippy::too_many_arguments)]
pub(super) async fn disk_policy(
    store: &JobStorage,
    consumer_id: &str,
    kind: &str,
    hostname: &str,
    fleet_staging: &Option<String>,
    tick_store_deadline: Instant,
    total_vram_gb: i64,
    slots: &[ActiveSlot],
    agent_diag: &mut Map<String, Value>,
    disk_low_bytes: &mut Option<i64>,
    last_cap: &mut Option<CapacitySnapshot>,
    last_fleet_flush: &mut Instant,
    log_fn: &mut dyn FnMut(&str),
) -> anyhow::Result<Step<DiskPolicy>> {
    let store_budget_left = || tick_store_deadline.saturating_duration_since(Instant::now());
    // Admission reads the canonical declaration directly as well as the
    // janitor report. Cleanup deliberately uses a cross-process lock; a
    // busy lock or an older writer's invalid report must not erase a
    // perfectly readable low watermark and close the queue forever.
    //
    // Bounded on the same budget, and a lapsed budget is NOT an error: the
    // registry states the watermark, the pinned-only flag and the VRAM
    // override, and this tick keeps whatever it last knew of all three
    // (`disk_low_bytes` from the janitor's state file, `pinned_only` from
    // the previous tick) rather than spending the broadcast's freshness
    // window waiting for a restatement. `?` still propagates a real
    // refusal, which is a different fact from a slow route: the registry
    // fetch already falls back to its last-known-good copy and to the
    // bundled snapshot before it errors at all.
    let registry_target =
        match tokio::time::timeout(store_budget_left(), lookup_self_auto(hostname)).await {
            Ok(result) => result?,
            Err(_) => {
                log_fn(&format!(
                    "loop: canonical registry target did not answer within {}s; keeping the last \
                     known disk watermark and pinned-only state for this tick",
                    constants::AGENT_TICK_STORE_BUDGET_S
                ));
                None
            }
        };
    if let Some(declared_low) = registry_target
        .as_ref()
        .and_then(|target| target.disk_cleanup.as_ref())
        .map(|policy| policy.low_free_gb.saturating_mul(disk_cleanup::GIB))
    {
        if *disk_low_bytes != Some(declared_low) {
            log_fn("loop: loaded disk low watermark from the canonical registry");
        }
        *disk_low_bytes = Some(declared_low);
    }
    // Python: shutil.disk_usage(expanduser("~")).free, OSError -> None.
    let current_free_bytes = disk_cleanup::free_bytes(&crate::config_file::expand_tilde("~")).ok();
    // Two different questions used to share one answer, and the conflation
    // is what froze the always-on mac. "Can this agent read its disk policy
    // at all" is a reason to fail admission closed: an agent that does not
    // know its own threshold cannot judge anything. "Is free space below the
    // janitor's low watermark" is not that. It is the janitor's cue to start
    // deleting, and on a host whose cleaners have nothing eligible to delete
    // -- every cleaner on that mac reported zero eligible items -- it is a
    // condition no cleanup pass can clear, so treating it as an admission
    // gate stopped the host permanently and silently: 19.6 GiB free against
    // a 20 GiB watermark, a zero-capacity publish, `continue`, forever.
    //
    // So pressure no longer suppresses the BROADCAST. It still suppresses
    // claiming, and the first version of this change did not, which was
    // wrong: within forty minutes of the same host being put back on the
    // fleet store its free space fell 19.3 -> 17.0 -> 13.8 GiB, because the
    // queue it had started draining is full of `cargo build` workloads and
    // the jobs themselves are what consume the disk. The gates that measure
    // actual consumption do not cover them -- the `$HOME` write probe only
    // fails once the disk is already full, and the raw-disk reserve applies
    // to activation-extraction jobs alone -- so removing the watermark from
    // admission would have let the host claim its way to zero.
    //
    // The defect was never that pressure stops claiming. It was that a host
    // which stops claiming says nothing at all: the broadcast went to zero
    // and the row went stale, so the fleet could not distinguish "under its
    // disk watermark" from "dead". Capacity is now published every loop with
    // `disk_pressure_active` in the diagnostics, and `host gates` reports the
    // numbers, so the operator gets a reason instead of a silence.
    let disk_policy_known = disk_low_bytes.is_some();
    let readings_incomplete = disk_low_bytes.is_none() || current_free_bytes.is_none();
    let pressure_active = disk_cleanup::disk_pressure_active(*disk_low_bytes, current_free_bytes);
    agent_diag.insert(
        "disk_cleanup_policy_known".into(),
        Value::from(disk_policy_known),
    );
    // The key keeps its published name: `host gates` reads it to say the
    // agent is refusing to claim because it cannot read its disk policy, and
    // that is now exactly what it means and nothing more.
    agent_diag.insert(
        "disk_pressure_unresolved".into(),
        Value::from(readings_incomplete),
    );
    agent_diag.insert("disk_pressure_active".into(), Value::from(pressure_active));
    if readings_incomplete {
        let snapshot = measured_capacity(
            slots,
            false,
            Some("disk_policy_unreadable"),
            BTreeMap::new(),
            0,
            total_vram_gb,
            agent_diag.clone(),
        );
        log_fn(&format!(
            "loop: disk-policy-unreadable: low watermark {} and free space {} -- failing \
             admission closed until both are known",
            disk_low_bytes.map_or("unknown".to_string(), |bytes| bytes.to_string()),
            current_free_bytes.map_or("unknown".to_string(), |bytes| bytes.to_string())
        ));
        let _ = publish_branch(
            store,
            consumer_id,
            kind,
            "disk-policy-unreadable",
            &snapshot,
            log_fn,
        )
        .await;
        *last_cap = Some(snapshot);
        tokio::time::sleep(Duration::from_secs(POLL_INTERVAL_S)).await;
        return Ok(Step::Done);
    }
    // The republish keep-alive below is unchanged from Python.
    if let Some(cap) = last_cap {
        let _ = publish_branch(
            store,
            consumer_id,
            kind,
            "keep-alive-republish",
            cap,
            log_fn,
        )
        .await;
    }
    if last_fleet_flush.elapsed() > Duration::from_secs(constants::FLEET_FLUSH_INTERVAL_S)
        && slots.is_empty()
    {
        if let Some(fleet_staging) = fleet_staging.as_deref() {
            if spawn_fleet_flush(Path::new(fleet_staging), log_fn).await? {
                log_fn("optional Hugging Face staging flush running asynchronously");
            }
        }
        *last_fleet_flush = Instant::now();
    }
    Ok(Step::Go((
        registry_target,
        current_free_bytes,
        pressure_active,
    )))
}
