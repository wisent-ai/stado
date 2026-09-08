//! What the report MEANS: whether a holder is this unit's own process, one
//! verdict per declared port, the one word for the whole unit, and the
//! sentence an operator acts on.
//!
//! Everything here reads the report the host already decided to send. It runs
//! on this side, not on the host, so it can be exercised against the shapes a
//! real fleet produces.

use super::*;

mod json_report;

#[cfg(test)]
mod ownership_and_unknowns;

pub use json_report::to_report;

/// Whether this pid, or the job that owns it, is the unit under test.
///
/// The label is authoritative. The launchd pid is accepted as well because a
/// job whose program `exec`s in place keeps the pid launchd recorded, and on
/// that path the label lookup and the pid agree; where they disagree the label
/// wins, which is what stops one of two units running identical argv from
/// claiming the other's process.
fn belongs_to_unit(holder: &Holder, unit: &str, launchd_pid: &str) -> bool {
    if holder.owner_state == OWNER_RESOLVED {
        return holder.owner == unit;
    }
    !launchd_pid.is_empty() && holder.pid == launchd_pid
}

/// Judge every port the caller declared.
pub fn port_verdicts(report: &ServingReport) -> Vec<PortVerdict> {
    report
        .ports
        .iter()
        .map(|port| {
            let verdict = if report.listeners_state != LISTENERS_READ {
                PORT_UNKNOWN
            } else if port.holders.is_empty() {
                PORT_DEAD
            } else if port
                .holders
                .iter()
                .any(|holder| belongs_to_unit(holder, &report.unit, &report.launchd_pid))
            {
                PORT_SERVED_BY_UNIT
            } else if port
                .holders
                .iter()
                .all(|holder| holder.owner_state == OWNER_UNKNOWN)
            {
                PORT_OWNER_UNKNOWN
            } else {
                PORT_SERVED_BY_OTHER
            };
            PortVerdict {
                port: port.port,
                verdict,
                holders: port.holders.clone(),
            }
        })
        .collect()
}

/// The one word for the whole unit.
///
/// `serving` requires every declared port to be held by this unit's own
/// process. Anything unreadable is `unknown` rather than either of the other
/// two, and a port held by another job is `not_serving` — the case that used
/// to read as `runs`.
pub fn verdict(report: &ServingReport, ports: &[PortVerdict]) -> &'static str {
    if report.listeners_state != LISTENERS_READ {
        return SERVING_UNKNOWN;
    }
    if ports.is_empty() {
        return SERVING_UNKNOWN;
    }
    if ports
        .iter()
        .any(|port| matches!(port.verdict, PORT_SERVED_BY_OTHER | PORT_DEAD))
    {
        return SERVING_NO;
    }
    if ports.iter().any(|port| port.verdict != PORT_SERVED_BY_UNIT) {
        return SERVING_UNKNOWN;
    }
    SERVING_YES
}

/// Why this unit is not serving, in the operator's words, or `None`.
pub fn failure(host: &str, report: &ServingReport, ports: &[PortVerdict]) -> Option<String> {
    match verdict(report, ports) {
        SERVING_YES => None,
        SERVING_UNKNOWN if report.listeners_state != LISTENERS_READ => Some(format!(
            "{host}: the socket table could not be read, so no declared port below was judged"
        )),
        SERVING_UNKNOWN if ports.is_empty() => Some(format!(
            "{host}: {} declares no loopback port, so whether it serves cannot be decided here",
            report.unit
        )),
        SERVING_UNKNOWN => Some(format!(
            "{host}: something answers on {}'s declared port(s) and which launchd job owns it \
             could not be established over this channel",
            report.unit
        )),
        _ => {
            let taken: Vec<String> = ports
                .iter()
                .filter(|port| port.verdict == PORT_SERVED_BY_OTHER)
                .map(|port| format!("{} is held by {}", port.port, port.holder_cell()))
                .collect();
            let dead: Vec<String> = ports
                .iter()
                .filter(|port| port.verdict == PORT_DEAD)
                .map(|port| port.port.to_string())
                .collect();
            let mut said = format!("{host}: {} is not serving", report.unit);
            if !taken.is_empty() {
                said.push_str(&format!(" — {}", taken.join("; ")));
            }
            if !dead.is_empty() {
                said.push_str(&format!(" — nothing is listening on {}", dead.join(", ")));
            }
            Some(said)
        }
    }
}
