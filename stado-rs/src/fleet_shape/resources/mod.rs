//! What a host actually serves and holds: the loopback listeners behind its
//! declared ports, the artefacts its service units execute, the free space its
//! policy obliges it to keep, and the stores this control plane depends on.
//!
//! The listener read is first because it returns the inventory the artefact
//! check consumes, so one round trip answers both.

pub(in crate::fleet_shape) mod artefacts;
pub(in crate::fleet_shape) mod disk;
pub(in crate::fleet_shape) mod stores;

use std::collections::BTreeMap;

use serde_json::Value;

use super::{Finding, PORT_CHECK};
use crate::deploy::Runner;
use crate::targets::{ComputeTarget, Registry};

/// One process per declared service port, and one health verdict that agrees
/// with the routes behind it.
///
/// Both come out of the same inventory read: it reports the loopback listeners
/// a host holds and the service directory it is supposed to satisfy.
/// Returns the inventory it read, so a second question about the same host is
/// a second judgement rather than a second round trip.
pub(in crate::fleet_shape) async fn listener_count(
    registry: &Registry,
    target: &ComputeTarget,
    runner: &Runner,
    out: &mut Vec<Finding>,
) -> Option<Value> {
    let reading = match crate::deploy::host_inventory::inventory_target(
        target,
        registry.service_directory.as_ref(),
        runner,
    )
    .await
    {
        Ok(reading) => reading,
        Err(error) => {
            out.push(Finding {
                check: PORT_CHECK,
                subject: target.name.clone(),
                declared: "the host answers a listener inventory".to_string(),
                observed: format!("inventory failed: {error}"),
                command: format!("stado host inventory {}", target.name),
            });
            return None;
        }
    };
    let listeners = reading
        .get("listeners")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let state = reading
        .get("listeners_state")
        .and_then(Value::as_str)
        .unwrap_or("");
    // A table nobody could read is not an empty table. Without this the check
    // would report "one listener per port" on a host whose `lsof` failed,
    // which is the exact false pass this module exists to refuse.
    if listeners.is_empty() && state != crate::deploy::host_inventory::LISTENERS_READ {
        out.push(Finding {
            check: PORT_CHECK,
            subject: target.name.clone(),
            declared: "the host reports its listeners".to_string(),
            observed: format!("listener table unread ({state})"),
            command: format!("stado host inventory {}", target.name),
        });
        return Some(reading);
    }
    let mut holders: BTreeMap<u64, Vec<String>> = BTreeMap::new();
    for listener in &listeners {
        let Some(port) = listener.get("port").and_then(Value::as_u64) else {
            continue;
        };
        let who = format!(
            "{}:{port} pid {}",
            listener
                .get("address")
                .and_then(Value::as_str)
                .unwrap_or("?"),
            listener.get("pid").and_then(Value::as_u64).unwrap_or(0),
        );
        holders.entry(port).or_default().push(who);
    }
    for (port, who) in holders {
        if who.len() < 2 {
            continue;
        }
        out.push(Finding {
            check: PORT_CHECK,
            subject: format!("{}:{port}", target.name),
            declared: "one process serves one declared port".to_string(),
            observed: format!("{} processes hold it: {}", who.len(), who.join(" | ")),
            command: format!(
                "stado service list --undeclared --host {0} and stado service reap --host {0} --command <substring> (report first, then --apply)",
                target.name
            ),
        });
    }
    Some(reading)
}
