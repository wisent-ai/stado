//! Reconciling what the file declares against what is actually listening.
//!
//! One row per effective assignment that names an endpoint, with the verdict
//! and the processes holding the port. The state of the socket table decides
//! whether a verdict may be given at all.

use super::super::*;
use super::{declared_endpoint, effective_text, shadowing};

/// One declared endpoint's verdict against the socket table, and every
/// process holding that port.
///
/// `listeners_state` is not decoration, for the reason
/// [`super::host_inventory::verdict`](crate::deploy::host_inventory::verdict) states: "nothing is listening" and
/// "nobody could ask" are opposite findings that look identical in an empty
/// list, and calling the second one dead reports a working host as broken.
pub fn endpoint_verdict(
    endpoint: Endpoint,
    listeners: &[ProcListener],
    listeners_state: &str,
) -> (&'static str, Vec<String>) {
    if !endpoint.loopback {
        return (ENDPOINT_REMOTE, Vec::new());
    }
    if listeners_state != LISTENERS_READ && listeners_state != LISTENERS_READ_WITHOUT_NAMES {
        return (ENDPOINT_UNKNOWN, Vec::new());
    }
    let holders: Vec<String> = listeners
        .iter()
        .filter(|listener| listener.port == endpoint.port)
        .map(|listener| {
            if listener.process.is_empty() {
                format!("pid {}", listener.pid)
            } else {
                format!("{} (pid {})", listener.process, listener.pid)
            }
        })
        .collect();
    if holders.is_empty() {
        (ENDPOINT_DEAD, holders)
    } else {
        (ENDPOINT_LISTENING, holders)
    }
}

/// One row of the endpoint reconciliation: a key, what it declares, and
/// whether anything answers there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointRow {
    pub key: String,
    pub line: u32,
    pub declared: String,
    pub port: u32,
    pub verdict: &'static str,
    pub holders: Vec<String>,
}

/// Reconcile every endpoint the file's EFFECTIVE assignments declare.
///
/// Only the effective assignment of each key is judged: a shadowed line is
/// not what the process runs with, and failing the command on a dead endpoint
/// nothing reads would be a false alarm. The shadowed lines are still reported
/// — by [`shadowing`], where they belong.
pub fn endpoint_rows(report: &EnvFileReport) -> Vec<EndpointRow> {
    let roles = shadowing(&report.entries);
    let mut rows = Vec::new();
    for (entry, role) in report.entries.iter().zip(roles) {
        if role != EFFECTIVE || entry.value_state == VALUE_REDACTED {
            continue;
        }
        let Some(endpoint) = declared_endpoint(&entry.key, &entry.value) else {
            continue;
        };
        let (verdict, holders) =
            endpoint_verdict(endpoint, &report.listeners, &report.listeners_state);
        rows.push(EndpointRow {
            key: entry.key.clone(),
            line: entry.line,
            declared: effective_text(&entry.value).to_string(),
            port: endpoint.port,
            verdict,
            holders,
        });
    }
    rows
}
