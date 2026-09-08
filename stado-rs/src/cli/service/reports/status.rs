//! `service status NAME`: the beacon rows for one unit, and — for a row the
//! beacon calls `failed` — the best-effort reads from the host that say why.

use super::*;

pub(crate) async fn status(name: &str, json: bool) -> Result<(), CmdError> {
    let store = beacon_store().await?;
    let rows = service::find_services(&store, name).await.map_err(click)?;
    if rows.is_empty() {
        return Err(unmanaged(name, None));
    }
    // A `failed` row is the beacon saying "it died" — more often than not
    // with an empty detail. The why lives on the host: launchd's last exit
    // status, and the stderr the unit wrote before it went. Gather it
    // best-effort over the read-only channels; `status` must still answer
    // when the host cannot.
    let runner = production_runner();
    let mut failures: Vec<FailureEvidence> = Vec::new();
    for row in &rows {
        if row.state == service::STATE_FAILED {
            failures.push(failure_evidence(row, &runner).await);
        }
    }
    render_status(&rows, json, &failures)
}

/// How many stderr lines one `failure:` block may carry.
const FAILURE_STDERR_LINES: usize = 10;

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

async fn failure_evidence(row: &ServiceStatus, runner: &crate::deploy::Runner) -> FailureEvidence {
    let unit = row.service.unit_id().to_string();
    let mut evidence = FailureEvidence {
        host: row.service.host.clone(),
        unit: unit.clone(),
        last_exit: None,
        error_origin: None,
        error_lines: Vec::new(),
        note: None,
    };
    // The last exit status rides the approved read-only allowlist: the
    // exact `launchctl list` entry, never a shell.
    let words = vec!["launchctl".to_string(), "list".to_string()];
    match host_exec::exec_host(&row.service.host, &words, runner).await {
        Ok(report)
            if report.get("status").and_then(Value::as_str) == Some(host_exec::OK_STATUS) =>
        {
            let stdout = report
                .get("stdout")
                .and_then(Value::as_str)
                .unwrap_or_default();
            evidence.last_exit = launchctl_last_exit(stdout, &unit);
        }
        Ok(report) => {
            let status = report
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            evidence.push_note(format!("last exit unreadable: {status}"));
        }
        Err(exc) => evidence.push_note(format!("last exit unreadable: {exc}")),
    }
    // The stderr tail comes from the same logs path `service logs` uses,
    // narrowed to the lines a failure block can show.
    match host_channel::canonical_target(&row.service.host).await {
        Ok(target) => {
            match service::tail_logs(&target, &row.service, 2 * FAILURE_STDERR_LINES, runner).await
            {
                Ok(log) => {
                    evidence.error_origin = log.error_origin;
                    evidence.error_lines = log
                        .error_body
                        .lines()
                        .take(FAILURE_STDERR_LINES)
                        .map(str::to_string)
                        .collect();
                }
                Err(exc) => evidence.push_note(format!("stderr unreadable: {exc}")),
            }
        }
        Err(exc) => evidence.push_note(format!("stderr unreadable: {exc}")),
    }
    evidence
}
