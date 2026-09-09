//! The operator's answer: the `STADO_*` marker lines folded into one report,
//! read back against the plan that produced them.

use serde_json::{json, Map, Value};

use super::plan::plan_agents;
use super::{
    AGENT_MISSING_PLIST, AGENT_NEEDS_PRIVILEGE, AGENT_NOT_LOADED, AGENT_RESTARTED, STATUS_BLOCKED,
    STATUS_OK,
};
use crate::deploy::{py_str_repr, DeployError};
use crate::targets::ComputeTarget;

/// Python `_parse_output`: fold the `STADO_*` marker lines of stdout into
/// the report dict. `Err` on a disk field that is not an integer (Python's
/// `int(fields[3])` raising ValueError), and `Err` naming the host when the
/// field arrived empty because `df` on the host answered nothing — the pass
/// used to print a zero there, which reads as a full disk.
///
/// Then the part Python never had: the per-unit words are read back against
/// the plan that produced them ([`account_for_agents`]), so a unit this pass
/// skipped or could not touch reaches the operator as an entry of its own
/// and moves the overall `status` off `ok`.
pub fn parse_output(stdout: &str, target: &ComputeTarget) -> Result<Value, DeployError> {
    let mut report = Map::new();
    report.insert("target".to_string(), json!(target.name));
    report.insert(
        "ssh".to_string(),
        target.ssh.as_ref().map_or(Value::Null, |ssh| json!(ssh)),
    );
    report.insert("status".to_string(), json!("failed"));
    report.insert("agents".to_string(), json!(Map::new()));
    report.insert("stable_binds".to_string(), json!(Map::new()));
    for line in stdout.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.first() == Some(&"STADO_AGENT") && fields.len() == 3 {
            report["agents"][fields[1]] = json!(fields[2]);
        } else if fields.first() == Some(&"STADO_STABLE_BIND") && fields.len() == 4 {
            // Keyed by the bind, not the product: the port is what every
            // consumer's configuration names and what an operator greps for
            // after reading a 503.
            report["stable_binds"][fields[2]] = json!({"product": fields[1], "verdict": fields[3]});
        } else if fields.first() == Some(&"STADO_DOMAIN") && fields.len() == 4 {
            report.insert(
                "launchd_domain".to_string(),
                json!({"name": fields[1], "status": fields[2], "reason": fields[3]}),
            );
        } else if fields.first() == Some(&"STADO_RECOVER")
            && fields.get(1) == Some(&"ok")
            && fields.len() == 6
        {
            let before = parse_disk_field(fields[3], "before the pass", fields[2])?;
            let after = parse_disk_field(fields[4], "after the pass", fields[2])?;
            report.insert("status".to_string(), json!("ok"));
            report.insert("host".to_string(), json!(fields[2]));
            report.insert("disk_free_kb_before".to_string(), json!(before));
            report.insert("disk_free_kb_after".to_string(), json!(after));
            report.insert("cleanup_status".to_string(), json!(fields[5]));
        } else if fields.first() == Some(&"STADO_CLEANUP") && fields.len() == 2 {
            let cleanup = serde_json::from_str(fields[1])
                .unwrap_or_else(|_| json!({"outcome": "invalid_output"}));
            report.insert("cleanup".to_string(), cleanup);
        } else if fields.first() == Some(&"STADO_RECOVER") {
            report.insert(
                "remote_error".to_string(),
                json!(fields[1..]
                    .iter()
                    .map(|f| f.to_string())
                    .collect::<Vec<_>>()),
            );
        }
    }
    account_for_agents(&mut report, target);
    Ok(Value::Object(report))
}

/// Read every managed unit's outcome back against the plan, and let the
/// overall `status` carry what happened to it.
///
/// `status: ok` used to mean nothing more than "the pass reached its last
/// line". On 2026-08-19 an operator ran this against control-host to get
/// the object API back, read `status: ok` with `launchd_domain: {name:
/// user/501, status: background}` underneath it, and reasonably concluded the
/// recovery had run. It had: it cleaned the disk, decommissioned the
/// coordinator, and did nothing whatsoever about the units it was asked to
/// re-bootstrap, because they are system daemons and it is not root.
///
/// So the facts it was hiding are now first-class:
///
/// - `skipped` — the unit is there, this pass may not load it, and the entry
///   names the privileged command that can.
/// - `blockers` — the unit cannot be loaded by anybody in its current state:
///   the declared file is absent, its scoped configuration is wrong, the
///   bootstrap failed, or the bootstrap succeeded and left no job.
///
/// And `launchd_domain: {status: background}` is no longer a note beside the
/// units it explains. When the pass could not load a declared agent in that
/// background domain, the blocker says so in the resolver's own words — no
/// graphical session, so no `gui/<uid>`, so nothing to load a LaunchAgent
/// into — because "the bootstrap failed" and "the bootstrap could not have
/// succeeded on this host today" send an operator to different places.
///
/// Either list being non-empty takes the status to [`STATUS_BLOCKED`], which
/// `cli/host.rs::recover` already turns into exit 1.
fn account_for_agents(report: &mut Map<String, Value>, target: &ComputeTarget) {
    let domain = report.get("launchd_domain").cloned().unwrap_or_default();
    let domain_field = |key: &str| -> String {
        domain
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let domain_name = domain_field("name");
    let domain_status = domain_field("status");
    let domain_reason = domain_field("reason");
    let mut skipped: Vec<Value> = Vec::new();
    let mut blockers: Vec<Value> = Vec::new();
    for plan in plan_agents(target) {
        let finding = report["agents"]
            .get(&plan.label)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        match finding.as_str() {
            // Nothing to account for: reloaded, or the pass stopped before it
            // reached the units at all (its own `status` already says why).
            "" | AGENT_RESTARTED => {}
            AGENT_NEEDS_PRIVILEGE => skipped.push(json!({
                "unit": plan.label,
                "reason": format!(
                    "declared at {} in launchd's system domain; the approved channel logs in as an \
                     unprivileged user and cannot bootstrap it. Re-bootstrap it on the host with: \
                     sudo launchctl kickstart -k system/{}",
                    plan.plist, plan.label
                ),
            })),
            AGENT_MISSING_PLIST => blockers.push(json!({
                "unit": plan.label,
                "finding": AGENT_MISSING_PLIST,
                "path": plan.plist,
                "reason": format!(
                    "the declared unit file {} is not on the host, so there is nothing to load and \
                     this host publishes no beacon. Reinstall it and load it with: sudo launchctl \
                     bootstrap system {}",
                    plan.plist, plan.plist
                ),
            })),
            other => {
                // Every remaining finding means the same thing about the host:
                // the unit is declared, this pass tried, and launchd has no job
                // under the label. In the background domain that is not bad
                // luck — it is the domain, and the reason belongs in the
                // blocker rather than in a note above it that nobody connected
                // to the unit underneath.
                let reason = if domain_status == crate::deploy::service::DOMAIN_STATUS_BACKGROUND {
                    format!(
                        "{} could not be loaded in {domain_name}, the only domain this login has: \
                         {domain_reason}. Until somebody is logged in graphically on this host, or \
                         the unit is declared in launchd's system domain, nothing will run it",
                        plan.label
                    )
                } else if other.starts_with(AGENT_NOT_LOADED) {
                    format!(
                        "the pass bootstrapped {} in {domain_name} and launchd has no job there, so \
                         the unit is not running",
                        plan.label
                    )
                } else {
                    format!(
                        "the recovery pass refused to load {} from {}; the finding is its own word \
                         for why, and the unit is not running",
                        plan.label, plan.plist
                    )
                };
                blockers.push(json!({
                    "unit": plan.label,
                    "finding": other,
                    "path": plan.plist,
                    "domain": domain_name,
                    "reason": reason,
                }));
            }
        }
    }
    let clean = skipped.is_empty() && blockers.is_empty();
    report.insert("skipped".to_string(), Value::Array(skipped));
    report.insert("blockers".to_string(), Value::Array(blockers));
    if !clean && report.get("status").and_then(Value::as_str) == Some(STATUS_OK) {
        report.insert("status".to_string(), json!(STATUS_BLOCKED));
    }
}

/// One free-space reading from the host, with the empty case named.
///
/// The remote program prints what `df` gave it. When `df` gave it nothing the
/// field is empty, and that is reported as the unreadable thing it is: the
/// program used to print a zero, which an operator reads as a disk with no
/// space left and which turned an unreadable host into a false emergency.
fn parse_disk_field(field: &str, moment: &str, host: &str) -> Result<i64, DeployError> {
    if field.is_empty() {
        return Err(DeployError(format!(
            "{host} reported no free-space reading for / {moment}: df answered nothing, so this \
             pass cannot say what the disk did"
        )));
    }
    parse_int_field(field)
}

/// Python `int(fields[i])` with the CPython ValueError message.
fn parse_int_field(field: &str) -> Result<i64, DeployError> {
    field.parse::<i64>().map_err(|_| {
        DeployError(format!(
            "invalid literal for int() with base 10: {}",
            py_str_repr(field)
        ))
    })
}

/// `json.dumps(report, indent=2, sort_keys=True)` as the CLI prints it.
pub fn to_sorted_pretty(value: &Value) -> String {
    fn sorted(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                Value::Object(
                    keys.into_iter()
                        .map(|key| (key.clone(), sorted(&map[key])))
                        .collect(),
                )
            }
            Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
            other => other.clone(),
        }
    }
    serde_json::to_string_pretty(&sorted(value)).expect("report serializes")
}
