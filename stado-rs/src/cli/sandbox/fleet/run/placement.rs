//! Where a fleet sandbox runs when the caller names no host: `create` puts
//! it on the registered Linux host with the most room for its declared
//! limits, every other operation finds the host that keeps it, and `list`
//! reads every host. A named `--host` is taken as it is, as before.
//!
//! Each decision is made from what every host answered about itself
//! ([`super::room`]) and printed with the receipt as `placement`, with the
//! verdict on each host, so a sandbox is never put on a host for a reason
//! nobody can read, and a refusal names every host and why it could not
//! serve.

use serde_json::{json, Value};

use super::room::{self, Reading, Room};
use crate::cli::CmdError;
use crate::deploy::host_channel;
use crate::targets::ComputeTarget;

/// The resources `create` declares, which the chosen host must have free.
pub(in crate::cli::sandbox::fleet) struct Demand {
    pub cpus: u32,
    pub memory_bytes: u64,
    pub gpus: u32,
}

/// What a sandbox operation runs against: the host and, when Stado chose
/// or found it, how.
pub(super) struct Placed {
    pub target: ComputeTarget,
    pub placement: Option<Value>,
}

/// The host named by `--host`, unchanged.
pub(super) async fn named(host: &str) -> Result<Placed, CmdError> {
    let registry = host_channel::canonical_registry().await?;
    let target = host_channel::resolve_target(&registry, host)?.clone();
    Ok(Placed {
        target,
        placement: None,
    })
}

/// Why `room` cannot take `demand`, or nothing when it can.
fn misfit(room: &Room, demand: &Demand) -> Option<String> {
    if !room.runsc {
        return Some("gVisor runsc is not registered in Docker".to_string());
    }
    if room.cgroup != "2" {
        return Some(format!("cgroup v2 is required; observed {}", room.cgroup));
    }
    if demand.gpus > 0 && !room.nvproxy {
        return Some(
            "runsc is registered without --nvproxy, so no GPU reaches a sandbox".to_string(),
        );
    }
    let cpus = u64::from(demand.cpus).saturating_mul(1_000_000_000);
    if room.free_nano_cpus() < cpus {
        return Some(format!(
            "{} nano-CPUs of its {} cores are not promised to running sandboxes; {} cores declared",
            room.free_nano_cpus(),
            room.cores,
            demand.cpus
        ));
    }
    if room.free_memory_bytes() < demand.memory_bytes {
        return Some(format!(
            "{} bytes of RAM are free for a sandbox; {} declared",
            room.free_memory_bytes(),
            demand.memory_bytes
        ));
    }
    if room.free_gpus() < u64::from(demand.gpus) {
        return Some(format!(
            "{} of its {} NVIDIA GPUs are free; {} declared",
            room.free_gpus(),
            room.gpus,
            demand.gpus
        ));
    }
    None
}

/// The verdict on each host, as printed in `placement.hosts`.
fn verdicts(readings: &[Reading], judge: impl Fn(&Reading) -> Value) -> Vec<Value> {
    readings.iter().map(judge).collect()
}

fn unread(readings: &[Reading]) -> Vec<String> {
    readings
        .iter()
        .filter_map(|reading| match &reading.answer {
            Err(error) => Some(format!("{}: {error}", reading.host)),
            Ok(_) => None,
        })
        .collect()
}

/// The host `create` puts sandbox `name` on: of the hosts whose answer
/// shows room for `demand`, the one with the most free RAM, then the most
/// free cores, then by name. A name another host already keeps is refused,
/// so a sandbox is addressed by its name alone; a host that could not be
/// read is named in the placement and does not stop the others.
pub(super) async fn place(name: &str, demand: &Demand) -> Result<Placed, CmdError> {
    let (targets, passed) = room::candidates().await?;
    let readings = room::read(&targets).await;
    if let Some(holder) = readings.iter().find(|reading| reading.holds(name)) {
        return Err(CmdError::refused(format!(
            "sandbox {name} already exists on {}; read it with sandbox fleet show {name}",
            holder.host
        )));
    }
    let chosen = readings
        .iter()
        .filter_map(|reading| match &reading.answer {
            Ok((room, _)) if misfit(room, demand).is_none() => Some((reading, room)),
            _ => None,
        })
        .max_by(|(left, a), (right, b)| {
            a.free_memory_bytes()
                .cmp(&b.free_memory_bytes())
                .then(a.free_nano_cpus().cmp(&b.free_nano_cpus()))
                .then(right.host.cmp(&left.host))
        })
        .map(|(reading, _)| reading.host.clone());
    let mut hosts = verdicts(&readings, |reading| match &reading.answer {
        Ok((room, _)) => match misfit(room, demand) {
            None => json!({"host": reading.host, "fits": true, "room": room}),
            Some(reason) => {
                json!({"host": reading.host, "fits": false, "reason": reason, "room": room})
            }
        },
        Err(error) => json!({"host": reading.host, "fits": false, "reason": error}),
    });
    hosts.extend(passed);
    let Some(chosen) = chosen else {
        let reasons = hosts
            .iter()
            .map(|verdict| {
                format!(
                    "{}: {}",
                    verdict["host"].as_str().unwrap_or_default(),
                    verdict["reason"].as_str().unwrap_or_default()
                )
            })
            .collect::<Vec<_>>();
        return Err(CmdError::refused(format!(
            "no registered Linux host has room for sandbox {name} ({} cores, {} bytes of RAM, {} GPUs); no sandbox was created: {}",
            demand.cpus,
            demand.memory_bytes,
            demand.gpus,
            if reasons.is_empty() { "the registry holds no host".to_string() } else { reasons.join("; ") }
        )));
    };
    let target = targets
        .into_iter()
        .find(|target| target.name == chosen)
        .expect("the chosen host is one of the candidates read");
    let placement = json!({"chosen": chosen,
        "demand": {"cpus": demand.cpus, "memory_bytes": demand.memory_bytes, "gpus": demand.gpus},
        "hosts": hosts});
    Ok(Placed {
        target,
        placement: Some(placement),
    })
}

/// The host that keeps sandbox `name`. None keeping it is `not_found`,
/// naming the hosts asked and those that did not answer; two keeping it is
/// refused, naming both, because an operation on the wrong one is not
/// undone. `fork` passes its new sandbox's name as `new`, refused when any
/// host already keeps it, so a name stays one sandbox across the fleet.
pub(super) async fn locate(name: &str, new: Option<&str>) -> Result<Placed, CmdError> {
    let (targets, _) = room::candidates().await?;
    let readings = room::read(&targets).await;
    if let Some((new, holder)) =
        new.and_then(|new| Some((new, readings.iter().find(|reading| reading.holds(new))?)))
    {
        return Err(CmdError::refused(format!(
            "sandbox {new} already exists on {}; nothing was forked",
            holder.host
        )));
    }
    let holders = readings
        .iter()
        .filter(|reading| reading.holds(name))
        .map(|reading| reading.host.clone())
        .collect::<Vec<_>>();
    let silent = unread(&readings);
    let placement = json!({"located": holders.first(),
    "hosts": verdicts(&readings, |reading| match &reading.answer {
        Ok(_) => json!({"host": reading.host, "holds": reading.holds(name)}),
        Err(error) => json!({"host": reading.host, "holds": null, "reason": error}),
    })});
    match holders.as_slice() {
        [host] => Ok(Placed {
            target: targets
                .into_iter()
                .find(|target| target.name == *host)
                .expect("the holding host is one of the candidates read"),
            placement: Some(placement),
        }),
        [] => Err(CmdError::missing(format!(
            "no registered Linux host keeps sandbox {name}; asked {}{}",
            readings
                .iter()
                .map(|reading| reading.host.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            if silent.is_empty() {
                String::new()
            } else {
                format!("; not read: {}", silent.join("; "))
            }
        ))),
        _ => Err(CmdError::refused(format!(
            "sandbox {name} exists on {}; name the one meant with --host",
            holders.join(" and ")
        ))),
    }
}

/// `list` with no host: every host's fleet sandboxes, each carrying the
/// host that keeps it. The listing is printed whole; a host that could not
/// be read is named in it and fails the command, so a partial listing is
/// never taken for the whole fleet.
pub(super) async fn list_fleet() -> Result<(), CmdError> {
    let (targets, passed) = room::candidates().await?;
    let readings = room::read(&targets).await;
    let sandboxes = readings
        .iter()
        .filter_map(|reading| {
            reading
                .answer
                .as_ref()
                .ok()
                .map(|(_, held)| (reading, held))
        })
        .flat_map(|(reading, held)| {
            held.iter().map(|entry| {
                let mut entry = entry.clone();
                entry["host"] = json!(reading.host);
                entry
            })
        })
        .collect::<Vec<_>>();
    let silent = unread(&readings);
    let mut hosts = verdicts(&readings, |reading| match &reading.answer {
        Ok((room, held)) => {
            json!({"host": reading.host, "read": true, "sandboxes": held.len(), "room": room})
        }
        Err(error) => json!({"host": reading.host, "read": false, "reason": error}),
    });
    hosts.extend(passed);
    let report = json!({"schema": "stado.fleet-sandbox.v1", "host": null, "operation": "list",
        "ok": silent.is_empty(), "result": {"sandboxes": sandboxes}, "placement": {"hosts": hosts}});
    println!("{}", serde_json::to_string_pretty(&report)?);
    if silent.is_empty() {
        Ok(())
    } else {
        Err(CmdError::unreachable(format!(
            "the fleet listing lacks hosts that could not be read: {}",
            silent.join("; ")
        )))
    }
}
