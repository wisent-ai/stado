//! `service watch-spawn`.

use super::*;

/// `service watch-spawn --host HOST --command SUBSTRING` — name the parent of
/// the next matching process, while that parent still exists.
///
/// The report leads with the parent because the parent is the whole question.
/// A row reading `ppid 1` with `launchd` as the parent is not a failure of
/// this command: it means the arrival was already reparented when the sample
/// caught it, and the answer is to sample faster.
pub(crate) async fn watch_spawn(
    host: &str,
    command: &str,
    seconds: u64,
    interval_ms: u64,
    json: bool,
) -> Result<(), CmdError> {
    let target = host_channel::canonical_target(host).await.map_err(click)?;
    let runner = production_runner();
    let report = service_spawn_watch::watch_spawns(&target, command, seconds, interval_ms, &runner)
        .await
        .map_err(click)?;
    if json {
        return print_json(&json!({
            "host": report.host,
            "matched": report.matched,
            "seconds": report.seconds,
            "interval_ms": report.interval_ms,
            "samples": report.samples,
            "elapsed_seconds": report.elapsed_seconds,
            "unsupported": report.unsupported,
            "baseline": report.baseline.iter().map(|entry| entry.row.to_json()).collect::<Vec<_>>(),
            "arrivals": report.arrivals.iter().map(service_spawn_watch::Arrival::to_json).collect::<Vec<_>>(),
        }));
    }
    if let Some(system) = &report.unsupported {
        println!(
            "{}: spawn watch is Darwin-only; the host reports {system}",
            report.host
        );
        return Ok(());
    }
    let baseline: Vec<Vec<String>> = report
        .baseline
        .iter()
        .map(|entry| {
            vec![
                entry.row.pid.clone(),
                entry.row.ppid.clone(),
                entry.row.started_at.clone(),
                entry.row.command.clone(),
            ]
        })
        .collect();
    println!("already running when the watch opened:");
    table::print(&["PID", "PPID", "STARTED_AT", "COMMAND"], &baseline);
    if report.arrivals.is_empty() {
        // A watch that saw nothing is a result, not a dud: it says the thing
        // that was respawning has stopped, or never fired in this window.
        println!(
            "\n{}: no process matching {:?} started in {}s across {} samples",
            report.host, report.matched, report.elapsed_seconds, report.samples
        );
        return Ok(());
    }
    for arrival in &report.arrivals {
        println!(
            "\narrival {} at +{}s: pid {} ppid {} — {}",
            arrival.sequence,
            arrival.after_seconds,
            arrival.row.pid,
            arrival.row.ppid,
            arrival.row.command
        );
        let cells: Vec<Vec<String>> = arrival
            .ancestry
            .iter()
            .map(|ancestor| {
                vec![
                    ancestor.depth.to_string(),
                    ancestor.row.pid.clone(),
                    ancestor.row.ppid.clone(),
                    if ancestor.alive { "yes" } else { "no" }.to_string(),
                    ancestor.row.started_at.clone(),
                    ancestor.row.command.clone(),
                ]
            })
            .collect();
        table::print(
            &["DEPTH", "PID", "PPID", "ALIVE", "STARTED_AT", "COMMAND"],
            &cells,
        );
        match arrival.parent() {
            Some(parent) => println!(
                "  parent: pid {} ({}) — {}",
                parent.row.pid,
                if parent.alive {
                    "still running"
                } else {
                    "already exited"
                },
                parent.row.command
            ),
            None => println!("  parent: not in the snapshot that caught it; sample faster"),
        }
    }
    println!(
        "\n{}: {} arrival(s) in {}s across {} samples",
        report.host,
        report.arrivals.len(),
        report.elapsed_seconds,
        report.samples
    );
    Ok(())
}
