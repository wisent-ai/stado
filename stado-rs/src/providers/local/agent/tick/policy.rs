//! The canonical registry target, the disk-full rule's verdict on the volume
//! the fleet writes to, and the two things a tick does once both are known:
//! republish, and flush staging.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use crate::primitives::constants;
use crate::providers::local::disk::fleet_flush::spawn_fleet_flush;
use crate::providers::local::disk_cleanup;
use crate::providers::local::slots::ActiveSlot;
use crate::queue::capacity::CapacitySnapshot;
use crate::queue::JobStorage;
use crate::targets::ComputeTarget;

use super::super::capacity::snapshot::{measured_capacity, publish_branch};
use super::super::{lookup_self_auto, Step};

/// What one tick learns about the disk it admits against: the canonical
/// registry target, the free bytes measured on the fleet's volume, and
/// whether that volume is at the disk-full threshold.
pub(super) type DiskPolicy = (Option<ComputeTarget>, Option<i64>, bool);

/// Read the disk this tick admits against, and report
/// `(registry target, free bytes, pressure active)`.
#[allow(clippy::too_many_arguments)]
pub(super) async fn disk_policy(
    store: &JobStorage,
    consumer_id: &str,
    kind: &str,
    hostname: &str,
    fleet_staging: &Option<String>,
    total_vram_gb: i64,
    slots: &[ActiveSlot],
    agent_diag: &mut Map<String, Value>,
    disk_low_bytes: &mut Option<i64>,
    last_cap: &mut Option<CapacitySnapshot>,
    last_fleet_flush: &mut Instant,
    log_fn: &mut dyn FnMut(&str),
) -> anyhow::Result<Step<DiskPolicy>> {
    // The registry fetch already falls back to its last-known-good copy and
    // to the bundled snapshot before it errors at all, so an error here is a
    // real refusal and ends the tick with it.
    let registry_target = lookup_self_auto(hostname).await?;
    // The work root is declared into this process once, from the target the
    // registry names for this host. A root that changes under a running
    // agent with live jobs is not followed: half the live job trees would sit
    // on each side of the move. With no job running there is nothing to
    // split, and the agent replaces its own process image with the same
    // binary and argv, the way a completed self-update does, so the new
    // root takes on the next start. Otherwise a host that declares a work
    // root on a large volume keeps measuring and writing the small root
    // volume indefinitely, logging every tick that the declaration will
    // take "when its unit restarts it" — and nothing in the fleet restarts
    // an agent's unit for a registry change.
    if let Some(root) = registry_target
        .as_ref()
        .and_then(|target| target.work_root.as_deref())
    {
        let declared = std::path::Path::new(root);
        if crate::providers::local::work_base::declare(declared) {
            log_fn(&format!(
                "loop: work root {root} declared by the canonical registry; job trees, build \
                 caches and the published free space are measured there"
            ));
        } else if crate::providers::local::work_base::declared().as_deref() != Some(declared) {
            if slots.is_empty() {
                log_fn(&format!(
                    "loop: the canonical registry now declares work root {root} and no job is \
                     running; restarting this agent's process onto it"
                ));
                let error = crate::self_update::reexec();
                log_fn(&format!(
                    "loop: could not restart onto work root {root}: {error}; this agent keeps \
                     the one it started with"
                ));
            } else {
                log_fn(&format!(
                    "loop: the canonical registry now declares work root {root}; this agent \
                     keeps the one it started with until its {} running job(s) finish",
                    slots.len()
                ));
            }
        }
    }
    // The volume the fleet writes to: the declared work root, or the home.
    // The disk-full rule judges it: at 80% used the janitor deletes
    // everything the fleet put here, and this agent claims no new work while
    // that is so, because the jobs themselves are what consume the disk.
    //
    // Pressure does not suppress the BROADCAST, only claiming: a host which
    // stops claiming must still say why, so capacity is published every loop
    // with `disk_pressure_active` in the diagnostics, and `host gates`
    // reports the numbers.
    let reading =
        disk_cleanup::rule::read_volume(&crate::providers::local::work_base::measured_volume())
            .ok();
    let current_free_bytes = reading.map(|reading| reading.free_bytes);
    *disk_low_bytes = reading.map(|reading| disk_cleanup::rule::reserve_bytes(reading.total_bytes));
    let pressure_active = reading.is_some_and(|reading| reading.full());
    agent_diag.insert(
        "disk_used_percent".into(),
        reading.map_or(Value::Null, |reading| Value::from(reading.used_percent())),
    );
    // The key keeps its published name: `host gates` reads it to say the
    // agent is refusing to claim because it cannot read its volume.
    agent_diag.insert(
        "disk_pressure_unresolved".into(),
        Value::from(reading.is_none()),
    );
    agent_diag.insert("disk_pressure_active".into(), Value::from(pressure_active));
    if reading.is_none() {
        let snapshot = measured_capacity(
            slots,
            false,
            Some("disk_unreadable"),
            BTreeMap::new(),
            0,
            total_vram_gb,
            agent_diag.clone(),
        );
        log_fn(
            "loop: disk-unreadable: the volume the fleet writes to could not be read -- failing \
             admission closed until it can",
        );
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
