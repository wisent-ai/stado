//! The verdict as `--json`, for a caller that wants the rows rather than the
//! sentence.

use serde_json::{json, Map, Value};

use super::super::*;
use crate::targets::ComputeTarget;

/// The report as `--json`, in [`super::host_inventory`](crate::deploy::host_inventory)'s report shape.
pub fn to_report(
    target: &ComputeTarget,
    report: &ServingReport,
    ports: &[PortVerdict],
    declared_owners: &dyn Fn(&str) -> bool,
) -> Map<String, Value> {
    let mut object = Map::new();
    object.insert("host".to_string(), json!(target.name));
    object.insert("unit".to_string(), json!(report.unit));
    object.insert("status".to_string(), json!(OK_STATUS));
    object.insert("loaded".to_string(), json!(report.loaded));
    object.insert("launchd_pid".to_string(), json!(report.launchd_pid));
    object.insert("listeners_state".to_string(), json!(report.listeners_state));
    object.insert("serving".to_string(), json!(verdict(report, ports)));
    object.insert(
        "ports".to_string(),
        Value::Array(
            ports
                .iter()
                .map(|port| {
                    json!({
                        "port": port.port,
                        "verdict": port.verdict,
                        "holders": port.holders.iter().map(|holder| json!({
                            "pid": holder.pid,
                            "comm": holder.comm,
                            "owner": holder.owner,
                            "owner_state": holder.owner_state,
                            "owner_declared": (holder.owner_state == OWNER_RESOLVED)
                                .then(|| declared_owners(&holder.owner)),
                        })).collect::<Vec<Value>>(),
                    })
                })
                .collect(),
        ),
    );
    object
}
