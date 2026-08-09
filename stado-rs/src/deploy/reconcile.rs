//! Close the gap between what the registry declares a host must run and what
//! that host actually runs.
//!
//! Every piece of this existed and none of them were joined up.
//! `ComputeTarget::managed_versions` is the declaration. `host inventory`
//! reads each host's installed binary and already computes the axis - behind,
//! ahead, mismatched, unjudged, undeclared. `host release` fetches an exact
//! coordinate, verifies its digest, stages it and repoints the active binary.
//! What no command did was walk the fleet, put those three together, and say
//! what is out of step.
//!
//! The consequence was not theoretical. A subcommand merged to `main` reached
//! no machine at all, and every host answered that its binary predated the
//! command, because nothing existed whose job was to notice.
//!
//! This deliberately does not reclassify anything. The verdicts come from
//! `host inventory`'s own report: a second implementation of "is this host
//! behind" is a second answer to one question, and the two would drift.
//!
//! Reporting and acting are separate. The default is a report, because an
//! operator has to be able to see drift without a machine changing under
//! them; delivery is a second, explicit act.

use serde_json::{json, Value};

use super::{host_inventory, DeployError, Runner};

/// The verdicts that mean the host is running what it was told to run.
///
/// Everything else is drift, including a verdict this code does not
/// recognise. Listing the good outcomes rather than the bad ones is what
/// stops a new verdict from being silently treated as healthy - which is
/// exactly what happened here: a MISSING binary reports `unknown`, that word
/// was on no list, and two hosts with no Skarbiec installed at all read as
/// "matches its declaration".
const SETTLED_VERDICTS: [&str; 2] = ["matched", "current"];

/// A declared binary that is not on the host. The most actionable state
/// there is, and the one an "is it the right version" question skips over.
const ABSENT: &str = "absent";

/// What one managed binary on one host is doing.
#[derive(Debug, Clone)]
pub struct Standing {
    pub binary: String,
    pub verdict: String,
    pub declared: String,
    pub installed: String,
}

/// One host's standing against its declaration.
#[derive(Debug, Clone)]
pub struct HostStanding {
    pub target: String,
    pub drift: Vec<Standing>,
    pub undeclared: Vec<String>,
    pub unreachable: Option<String>,
}

impl HostStanding {
    pub fn needs_delivery(&self) -> bool {
        self.drift
            .iter()
            .any(|entry| entry.verdict == "behind" || entry.verdict == ABSENT)
    }
}

fn text(entry: &Value, field: &str) -> String {
    entry
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Read one host and record where each managed binary stands.
///
/// The verdict is `host inventory`'s own, taken per binary from its report
/// rather than recomputed here. Two implementations of "is this host behind"
/// are two answers to one question, and they drift apart exactly when it
/// matters.
pub async fn examine(target_name: &str, runner: &Runner) -> HostStanding {
    match host_inventory::inventory_host(target_name, runner).await {
        Ok(report) => {
            let mut drift = Vec::new();
            let mut undeclared = Vec::new();
            let binaries = report
                .get("managed_binaries")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default();
            for entry in binaries {
                let verdict = text(entry, "version_verdict");
                let name = text(entry, "name");
                if verdict == "undeclared" {
                    undeclared.push(name);
                    continue;
                }
                if SETTLED_VERDICTS.contains(&verdict.as_str()) {
                    continue;
                }
                let state = text(entry, "state");
                let verdict = if state == "present" { verdict } else { ABSENT.to_string() };
                drift.push(Standing {
                    binary: name,
                    verdict,
                    declared: text(entry, "declared_version"),
                    installed: text(entry, "version"),
                });
            }
            HostStanding {
                target: target_name.to_string(),
                drift,
                undeclared,
                unreachable: None,
            }
        }
        Err(DeployError(detail)) => HostStanding {
            target: target_name.to_string(),
            drift: Vec::new(),
            undeclared: Vec::new(),
            unreachable: Some(detail),
        },
    }
}

/// The fleet's standing, as JSON an operator or another tool can read.
pub fn report(standings: &[HostStanding], applied: &[Value]) -> Value {
    let one = usize::from(u8::from(true));
    let mut drifting = usize::MIN;
    let mut unreachable = usize::MIN;
    let mut undeclared = usize::MIN;
    let hosts: Vec<Value> = standings
        .iter()
        .map(|standing| {
            if standing.unreachable.is_some() {
                unreachable = unreachable.saturating_add(one);
            }
            if !standing.drift.is_empty() {
                drifting = drifting.saturating_add(one);
            }
            undeclared = undeclared.saturating_add(standing.undeclared.len());
            json!({
                "target": standing.target,
                "unreachable": standing.unreachable,
                "undeclared": standing.undeclared,
                "drift": standing
                    .drift
                    .iter()
                    .map(|entry| json!({
                        "binary": entry.binary,
                        "verdict": entry.verdict,
                        "declared": entry.declared,
                        "installed": entry.installed,
                    }))
                    .collect::<Vec<Value>>(),
            })
        })
        .collect();
    json!({
        "summary": {
            "hosts": standings.len(),
            "drifting": drifting,
            "unreachable": unreachable,
            "undeclared_binaries": undeclared,
            "delivered": applied.len(),
        },
        "hosts": hosts,
        "delivered": applied,
    })
}
