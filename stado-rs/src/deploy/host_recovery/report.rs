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
    report.insert("ssh".to_string(), json!(target.ssh));
    report.insert("status".to_string(), json!("failed"));
    report.insert("agents".to_string(), json!(Map::new()));
    report.insert("stable_binds".to_string(), json!(Map::new()));
    for line in stdout.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields.as_slice() {
            ["STADO_AGENT", label, finding] => {
                report["agents"][*label] = json!(finding);
            }
            // Keyed by the bind, not the product: the port is what every
            // consumer's configuration names and what an operator greps for
            // after reading a 503.
            ["STADO_STABLE_BIND", product, bind, verdict] => {
                report["stable_binds"][*bind] = json!({"product": product, "verdict": verdict});
            }
            ["STADO_DOMAIN", name, status, reason] => {
                report.insert(
                    "launchd_domain".to_string(),
                    json!({"name": name, "status": status, "reason": reason}),
                );
            }
            ["STADO_RECOVER", "ok", host, before, after, cleanup_status] => {
                let before = parse_disk_field(before, "before the pass", host)?;
                let after = parse_disk_field(after, "after the pass", host)?;
                report.insert("status".to_string(), json!("ok"));
                report.insert("host".to_string(), json!(host));
                report.insert("disk_free_kb_before".to_string(), json!(before));
                report.insert("disk_free_kb_after".to_string(), json!(after));
                report.insert("cleanup_status".to_string(), json!(cleanup_status));
            }
            ["STADO_CLEANUP", printed] => {
                // An unreadable report keeps what the host printed and why it
                // did not parse; a bare "invalid_output" told nobody either.
                let cleanup = match serde_json::from_str::<Value>(printed) {
                    Ok(cleanup) => cleanup,
                    Err(error) => json!({
                        "outcome": "invalid_output",
                        "printed": printed,
                        "error": error.to_string(),
                    }),
                };
                report.insert("cleanup".to_string(), cleanup);
            }
            ["STADO_RECOVER", said @ ..] => {
                report.insert("remote_error".to_string(), json!(said));
            }
            _ => {}
        }
    }
    account_for_agents(&mut report, target);
    Ok(Value::Object(report))
}

/// Read every managed unit's outcome back against the plan, and let the
/// overall `status` carry what happened to it.
///
/// `status: ok` used to mean nothing more than "the pass reached its last
/// line". An operator running this against a control host to get the object
/// API back reads `status: ok` with `launchd_domain: {name: user/501,
/// status: background}` underneath it, and reasonably concludes the
/// recovery has run. It has: it cleaned the disk, decommissioned the
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
    // The launchd domain the pass reported; a pass that printed no domain
    // line names none, and the blockers say so instead of naming "".
    let domain = report.get("launchd_domain").cloned();
    let domain_field = |key: &str| -> Option<String> {
        domain
            .as_ref()
            .and_then(|domain| domain.get(key))
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let domain_name = domain_field("name");
    let domain_status = domain_field("status");
    let domain_reason = domain_field("reason");
    let domain_shown = match &domain_name {
        Some(name) => name.clone(),
        None => "the launchd domain this pass did not report".to_string(),
    };
    let mut skipped: Vec<Value> = Vec::new();
    let mut blockers: Vec<Value> = Vec::new();
    // Only a pass that reached its last line owes a word for every planned
    // unit; one that stopped earlier already says why in its own `status`.
    let completed = report.get("status").and_then(Value::as_str) == Some(STATUS_OK);
    for plan in plan_agents(target) {
        let finding = report["agents"]
            .get(&plan.label)
            .and_then(Value::as_str)
            .map(str::to_string);
        let Some(finding) = finding else {
            // A completed pass that names no outcome for a planned unit was
            // read as "nothing to account for", and the report said ok about
            // a unit nobody saw.
            if completed {
                blockers.push(json!({
                    "unit": plan.label,
                    "finding": "unreported",
                    "path": plan.plist,
                    "reason": format!(
                        "the recovery pass reached its last line and reported no outcome for {}, \
                         so whether it runs is unknown",
                        plan.label
                    ),
                }));
            }
            continue;
        };
        match finding.as_str() {
            AGENT_RESTARTED => {}
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
                let reason = if domain_status.as_deref() == Some(crate::deploy::service::DOMAIN_STATUS_BACKGROUND) {
                    let why = match &domain_reason {
                        Some(why) => why.clone(),
                        None => "the pass gave no reason".to_string(),
                    };
                    format!(
                        "{} could not be loaded in {domain_shown}, the only domain this login has: \
                         {why}. Until somebody is logged in graphically on this host, or \
                         the unit is declared in launchd's system domain, nothing will run it",
                        plan.label
                    )
                } else if other.starts_with(AGENT_NOT_LOADED) {
                    format!(
                        "the pass bootstrapped {} in {domain_shown} and launchd has no job there, so \
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
        return Err(DeployError::unreachable(format!(
            "{host} reported no free-space reading for / {moment}: df answered nothing, so this \
             pass cannot say what the disk did"
        )));
    }
    parse_int_field(field)
}

/// Python `int(fields[i])` with the CPython ValueError message.
fn parse_int_field(field: &str) -> Result<i64, DeployError> {
    field.parse::<i64>().map_err(|error| {
        DeployError::unreachable(format!(
            "invalid literal for int() with base 10: {} ({error})",
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
