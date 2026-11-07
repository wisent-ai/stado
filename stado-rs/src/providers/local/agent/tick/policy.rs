//! The canonical registry target, the volume the fleet writes to against the
//! room work needs there, and the two things a tick does once both are
//! known: republish, and flush staging.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value};

use crate::providers::local::disk::fleet_flush::spawn_fleet_flush;
use crate::providers::local::slots::ActiveSlot;
use crate::providers::local::space_reclaim;
use crate::queue::capacity::CapacitySnapshot;
use crate::queue::JobStorage;
use crate::targets::ComputeTarget;

use super::super::capacity::snapshot::{measured_capacity, publish_branch};
use super::super::{lookup_self_auto, Step};

/// What one tick learns about the disk it admits against: the canonical
/// registry target, and the sentence a refusal carries when the volume's free
/// bytes do not cover the room work here needs (`None` when they do).
pub(super) type DiskPolicy = (Option<ComputeTarget>, Option<String>);

/// Read the disk this tick admits against, and report
/// `(registry target, room short)`. `janitor_report` is the janitor's latest
/// completed pass report, whose margin is part of the room.
#[allow(clippy::too_many_arguments)]
pub(super) async fn disk_policy(
    store: &JobStorage,
    consumer_id: &str,
    kind: &str,
    hostname: &str,
    fleet_staging: &Option<String>,
    total_vram_gb: i64,
    slots: &[ActiveSlot],
    janitor_report: &Value,
    agent_diag: &mut Map<String, Value>,
    last_cap: &mut Option<CapacitySnapshot>,
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
    // Two decisions read it. The disk-full rule is the janitor's: at 80% used
    // it deletes everything the fleet put here, and this agent publishes that
    // the rule holds (`disk_pressure_active`). Whether work fits is
    // admission's: this agent claims work while the free bytes are above the
    // room work here needs (the largest release scratch recorded on this host
    // plus the janitor's margin), at any percentage used, and otherwise
    // refuses with `disk_room_short` naming both figures. A percentage of a
    // workstation's disk says nothing about whether the next job fits in the
    // hundreds of GiB it may still have free.
    //
    // Neither suppresses the BROADCAST, only claiming: a host which stops
    // claiming must still say why, so capacity is published every loop with
    // `disk_room` in the diagnostics, and `host gates` reports the numbers.
    let volume = crate::providers::local::work_base::measured_volume();
    // The cause is published with the capacity diagnostics, which carry no
    // local path, so it names the fleet volume rather than where it is.
    let (reading, unread) = match space_reclaim::rule::read_volume(&volume) {
        Ok(reading) => (Some(reading), None),
        Err(error) => (
            None,
            Some(format!("the fleet volume could not be read: {error}")),
        ),
    };
    let pressure_active = reading.is_some_and(|reading| reading.full());
    // The volume as measured, null with the cause beside it when it was not,
    // so a reader judges a build's need against the bytes themselves rather
    // than rebuilding them from a rounded percentage.
    agent_diag.insert(
        "disk_used_percent".into(),
        Value::from(reading.map(|reading| reading.used_percent())),
    );
    agent_diag.insert(
        "disk_total_bytes".into(),
        Value::from(reading.map(|reading| reading.total_bytes)),
    );
    agent_diag.insert(
        "disk_free_bytes".into(),
        Value::from(reading.map(|reading| reading.free_bytes)),
    );
    agent_diag.insert("disk_unread".into(), Value::from(unread));
    // The key keeps its published name: `host gates` reads it to say the
    // agent is refusing to claim because it cannot read its volume.
    agent_diag.insert(
        "disk_pressure_unresolved".into(),
        Value::from(reading.is_none()),
    );
    agent_diag.insert("disk_pressure_active".into(), Value::from(pressure_active));
    // The room, judged once the volume is read: what the free bytes are
    // measured against, published whole so every reader states the same two
    // figures, or why it could not be judged.
    let room_short = match reading {
        Some(reading) => {
            let room = space_reclaim::margin::Room::judge(reading.free_bytes, janitor_report);
            agent_diag.insert(
                space_reclaim::margin::ROOM_KEY.into(),
                match &room {
                    Ok(room) => room.to_json(),
                    Err(cause) => serde_json::json!({ "unread": cause }),
                },
            );
            space_reclaim::margin::admission(&room, reading)
        }
        None => {
            agent_diag.remove(space_reclaim::margin::ROOM_KEY);
            None
        }
    };
    agent_diag.insert(
        crate::deploy::host_gates::DISK_ROOM_SHORT.into(),
        Value::from(room_short.is_some()),
    );
    if reading.is_none() {
        let snapshot = measured_capacity(
            slots,
            false,
            Some("disk_unreadable"),
            BTreeMap::new(),
            None,
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
    // A flush starts whenever the host is idle and the previous one has
    // ended: the PID lock keeps it single-flight, so staged rows go up one
    // commit per finished flush, and a refusal that names `Retry-After` holds
    // the next one until the moment Hugging Face stated.
    if slots.is_empty() {
        if let Some(fleet_staging) = fleet_staging.as_deref() {
            spawn_fleet_flush(Path::new(fleet_staging), log_fn).await?;
        }
    }
    Ok(Step::Go((registry_target, room_short)))
}
