//! `service status NAME`: the beacon rows for one unit, and — for a row the
//! beacon calls `failed` — the best-effort reads from the host that say why.

use super::*;

pub(crate) async fn status(name: &str, lines: Option<usize>, json: bool) -> Result<(), CmdError> {
    let store = beacon_store().await?;
    let rows = service::find_services(&store, name).await.map_err(click)?;
    if rows.is_empty() {
        return Err(unmanaged(name, None));
    }
    // A failed unit's own final stderr line and exit are part of status,
    // even when the reader did not request a larger log window. The beacon
    // can be stale, but a host read names the process that actually exited.
    let runner = production_runner();
    let mut failures: Vec<FailureEvidence> = Vec::new();
    for row in &rows {
        if row.state == service::STATE_FAILED {
            failures.push(failure_evidence(&row.service, lines, &runner).await);
        }
    }
    render_status(&rows, json, &failures)
}

/// The `Status` column of one label in `launchctl list` output: launchd's
/// last exit status for the job while nothing runs under it. Columns are
/// PID, Status, Label, tab-separated; the header row never collides with a
/// real label.
fn launchctl_last_exit(stdout: &str, label: &str) -> Option<String> {
    stdout.lines().find_map(|line| {
        let mut columns = line.split('\t');
        let _pid = columns.next()?;
        let status = columns.next()?;
        let name = columns.next()?;
        (name == label).then(|| status.to_string())
    })
}

async fn failure_evidence(
    declared: &ManagedService,
    lines: Option<usize>,
    runner: &crate::deploy::Runner,
) -> FailureEvidence {
    let unit = declared.unit_id().to_string();
    let mut evidence = FailureEvidence {
        host: declared.host.clone(),
        unit: unit.clone(),
        last_exit: None,
        stderr_read: true,
        error_origin: None,
        error_lines: Vec::new(),
        note: None,
    };
    // The last exit status rides the approved read-only allowlist: the
    // exact `launchctl list` entry, never a shell.
    let words = vec!["launchctl".to_string(), "list".to_string()];
    match host_exec::exec_host(&declared.host, &words, runner).await {
        Ok(report)
            if report.get("status").and_then(Value::as_str) == Some(host_exec::OK_STATUS) =>
        {
            // An ok answer without its listing names no exit; read as empty
            // text it read as a unit launchctl does not list.
            match report.get("stdout").and_then(Value::as_str) {
                Some(stdout) => evidence.last_exit = launchctl_last_exit(stdout, &unit),
                None => evidence.push_note(format!(
                    "last exit unreadable: the host answered ok without the listing: {report}"
                )),
            }
        }
        // The host answered without an ok status: its whole answer is the
        // evidence, since no field of it was promised.
        Ok(report) => {
            evidence.push_note(format!("last exit unreadable: the host answered {report}"))
        }
        Err(exc) => evidence.push_note(format!("last exit unreadable: {exc}")),
    }
    // The default is the last stderr line; --lines requests a larger window.
    // tail_logs divides its window between stdout and stderr.
    let requested = match lines {
        Some(lines) => lines,
        None => std::num::NonZeroUsize::MIN.get(),
    };
    match crate::cli::canonical_host(&declared.host).await {
        Ok(target) => match service::tail_logs(
            &target,
            declared,
            requested.saturating_add(requested),
            runner,
        )
        .await
        {
            Ok(log) => {
                evidence.error_origin = log.error_origin;
                evidence.error_lines = if lines.is_some() {
                    log.error_body.lines().map(str::to_string).collect()
                } else {
                    log.error_body
                        .lines()
                        .last()
                        .map(str::to_string)
                        .into_iter()
                        .collect()
                };
            }
            Err(exc) => evidence.push_note(format!("stderr unreadable: {exc}")),
        },
        Err(exc) => evidence.push_note(format!("stderr unreadable: {exc}")),
    }
    evidence
}

/// Native exit and stderr evidence independent of a healthy beacon row.
/// Activation reads this before it restores a predecessor.
pub(crate) async fn last_failure(declared: &ManagedService) -> String {
    let evidence = failure_evidence(declared, None, &production_runner()).await;
    let mut parts = Vec::new();
    match evidence.last_exit {
        Some(exit) => parts.push(format!("last launchctl exit {exit}")),
        None => parts.push(format!(
            "{}: launchctl did not report the last exit",
            declared.unit_id()
        )),
    }
    match evidence.error_lines.last() {
        Some(line) => parts.push(format!("last stderr: {line}")),
        None => parts.push(format!(
            "{}: stderr had no readable last line",
            declared.unit_id()
        )),
    }
    if let Some(note) = evidence.note {
        parts.push(note);
    }
    parts.join("; ")
}
