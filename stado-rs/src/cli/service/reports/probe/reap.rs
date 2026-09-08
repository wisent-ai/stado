//! `service reap`.

use super::*;

/// `service reap --host HOST [--apply]` — end the product processes on HOST
/// that no declared unit owns.
///
/// Ownership is the registry's, not launchd's. `list --unowned` asks whether ANY
/// launchd job claims a process, and on a mac that set is about a thousand pids,
/// so a duplicate running under a label the document never declared reads as
/// owned and is left alone. This asks the question that matters: is this process
/// the one the document says should be running.
pub(crate) async fn reap(
    host: &str,
    command: &str,
    apply: bool,
    json: bool,
) -> Result<(), CmdError> {
    let target = host_channel::canonical_target(host).await.map_err(click)?;
    let runner = production_runner();
    let (reaped, kept) = service::reap_undeclared_processes(&target, command, apply, &runner)
        .await
        .map_err(click)?;
    if json {
        let payload: Vec<Value> = reaped.iter().map(service::ReapedProcess::to_json).collect();
        return print_json(&json!({
            "host": target.name,
            "applied": apply,
            "kept_pids": kept,
            "reaped": payload,
        }));
    }
    let cells: Vec<Vec<String>> = reaped
        .iter()
        .map(|process| {
            vec![
                process.pid.clone(),
                process.outcome.clone(),
                dash(&process.started_at),
                process.command.clone(),
            ]
        })
        .collect();
    table::print(&["PID", "OUTCOME", "STARTED_AT", "COMMAND"], &cells);
    // The kept set is the other half of the verdict: an empty table with no
    // kept pid means the declared units are not running either, which is a
    // different problem from a clean host.
    println!(
        "{}: declared units hold pid(s) [{}]{}",
        target.name,
        if kept.is_empty() { "none" } else { &kept },
        if apply {
            ""
        } else {
            "; nothing was signalled (pass --apply)"
        }
    );
    let stubborn = reaped
        .iter()
        .filter(|process| process.outcome == "still_running")
        .count();
    if stubborn > 0 {
        return Err(CmdError::click(format!(
            "{}: {stubborn} process(es) did not end on SIGTERM; their rows name each pid",
            target.name
        )));
    }
    Ok(())
}
