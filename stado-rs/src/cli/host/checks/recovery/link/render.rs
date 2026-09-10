use serde_json::Value;

use crate::cli::CmdError;
use crate::targets::ComputeTarget;

use crate::cli::host::checks::probes::cell;
use crate::cli::host::checks::recovery::link::{link_outcome, reason_counts, silence_instant};

/// The human half of [`super::report::link`], in the shape `host gates`
/// prints, so an operator reading one of these two commands can read the
/// other without relearning it.
#[allow(clippy::too_many_arguments)]
pub(super) fn render(
    resolved: &ComputeTarget,
    verdict: &str,
    blockers: Vec<String>,
    signal: &crate::deploy::host_state::ping::BeaconSignal,
    beacon_publisher: Option<Value>,
    ssh_error: Option<String>,
    connection_probe_error: Option<String>,
    connection_probes: Vec<crate::deploy::host_channel::SshConnectionProbe>,
    selected_connection: Option<String>,
    session: crate::deploy::service::HostSession,
    published: Option<Value>,
    path_kind: Value,
    changes: Vec<Value>,
    refused: bool,
    refusals: crate::monitor::host_silence::RefusalSummary,
    silences: Vec<crate::monitor::host_silence::SilenceRecord>,
) -> Result<(), CmdError> {
    // The same facts in the shape `host gates` prints, so an operator reading
    // one of these two commands can read the other without relearning it.
    println!("host:     {}", resolved.name);
    println!("verdict:  {verdict}");
    if blockers.is_empty() {
        println!("blockers: none");
    } else {
        // One per line, unabridged. These are whole sentences from the reader,
        // the channel and the host's own agent; comma-joining them made three
        // accounts read as one.
        for (index, blocker) in blockers.iter().enumerate() {
            let label = if index == usize::MIN {
                "blockers:"
            } else {
                "         "
            };
            println!("{label} {blocker}");
        }
    }
    match (signal.age_seconds, signal.reported_at.as_deref()) {
        (Some(age), Some(reported)) => println!(
            "beacon:   {} old, reported {reported}",
            crate::cli::registry::human_age(chrono::TimeDelta::seconds(age))
        ),
        _ => println!("beacon:   nothing readable for this host"),
    }
    if let Some(publisher) = &beacon_publisher {
        println!(
            "publisher:{}",
            publisher.get("detail").and_then(Value::as_str).map_or_else(
                || " no diagnosis".to_string(),
                |detail| format!(" {detail}")
            )
        );
    }
    println!(
        "ssh:      {}",
        match &ssh_error {
            None => "answered".to_string(),
            Some(detail) => format!("did not answer: {detail}"),
        }
    );
    if let Some(detail) = &connection_probe_error {
        println!("routes:   could not probe: {detail}");
    } else {
        for (index, probe) in connection_probes.iter().enumerate() {
            let label = if index == 0 { "routes:  " } else { "         " };
            let selected = if selected_connection.as_deref() == Some(probe.name.as_str()) {
                ", selected"
            } else {
                ""
            };
            let state = if probe.reachable {
                format!("answered{selected}")
            } else {
                format!(
                    "did not answer: {}",
                    probe.error.as_deref().unwrap_or("SSH probe failed")
                )
            };
            println!("{label} {} ({}) {state}", probe.name, probe.destination);
        }
    }
    // The headline in the operator's words first, the resolver's own sentence
    // under it. Reversing those two is how `gui/501` becomes the answer to
    // "is anyone logged in on that host".
    println!("session:  {}", session.headline());
    println!("          {}", session.detail);
    // "unknown" alone, not "unknown via -": a host that published no endpoint
    // has one fact to report, and a dash standing in for a second one reads as
    // a field that failed rather than a field that does not apply.
    println!(
        "path:     {}",
        match published.as_ref().and_then(|block| block.get("endpoint")) {
            Some(Value::String(endpoint)) => format!("{} via {endpoint}", cell(Some(&path_kind))),
            _ => cell(Some(&path_kind)),
        }
    );
    println!(
        "sleep:    last slept {}, last woke {}",
        cell(
            published
                .as_ref()
                .and_then(|block| block.get("last_sleep_at"))
        ),
        cell(
            published
                .as_ref()
                .and_then(|block| block.get("last_wake_at"))
        ),
    );
    if changes.is_empty() {
        println!("changes:  none recorded");
    } else {
        println!("changes:  {} recorded", changes.len());
        for change in &changes {
            println!(
                "          {} {}",
                cell(change.get("at")),
                cell(change.get("detail"))
            );
        }
    }
    if refused {
        println!(
            "refusals: {} in the last {}s: {}",
            refusals.count,
            refusals.window_seconds,
            reason_counts(&refusals)
        );
    } else {
        println!("refusals: none in the last {}s", refusals.window_seconds);
    }
    if silences.is_empty() {
        println!("silences: none recorded for this host");
    } else {
        println!("silences: {} recorded, newest first", silences.len());
        for record in &silences {
            println!(
                "          {} -> {} ({}){}",
                silence_instant(record.started_at),
                record
                    .ended_at
                    .map_or_else(|| "still open".to_string(), silence_instant),
                record
                    .duration_seconds
                    .map_or_else(|| "-".to_string(), |seconds| format!("{seconds}s")),
                record
                    .first_reader_error
                    .as_deref()
                    .map_or_else(String::new, |detail| format!(
                        ", first reader error: {detail}"
                    )),
            );
        }
    }
    link_outcome(&resolved.name, verdict, blockers.len())
}
