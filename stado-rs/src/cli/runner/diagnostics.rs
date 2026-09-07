//! `stado runner diagnostics` — what a runner that will not start is saying.
//!
//! A GitHub runner's own diagnosis is not in journald. It is in
//! `_diag/Runner_*.log` and `_diag/Worker_*.log` inside the runner root, which
//! is why `stado host unit-log <target> <unit>` answering `-- No entries --` is
//! perfectly consistent with a listener that is failing loudly. Until this
//! command existed the only product readers of that log were `runner status`
//! and `runner restart`, and both reduced it to the last line matching a fixed
//! pattern — so a .NET `System.IO.IOException: Permission denied`, whose
//! frames carry the path it could not open, reached an operator as the word
//! `Permission denied` and nothing else. That cost an hour on
//! 2026-09-07 against `ubuntu-server-rtx-pro-6000`.
//!
//! The tail is returned whole. `--json` carries it as a string so a console or
//! a report keeps the frames, and the human rendering prints them last, after
//! the unit's own verdict, because the verdict is one line and the log is many.

use serde_json::Value;

use super::{click, print_json, text};
use crate::cli::CmdError;

fn line(label: &str, value: Option<&Value>) {
    println!("{label:<14}{}", text(value));
}

pub(super) async fn render(target: &str, profile: &str, json: bool) -> Result<(), CmdError> {
    let report = crate::deploy::host_precheck_runner::diagnostics_declared(target, profile)
        .await
        .map_err(|error| click(error, json))?;
    if json {
        print_json(&report);
        return Ok(());
    }
    line("target", report.get("target"));
    line("profile", report.get("profile"));
    line("unit", report.get("unit"));
    line("account", report.get("account"));
    line("runner root", report.get("runner_root"));
    line("active", report.get("active_state"));
    line("sub", report.get("sub_state"));
    line("result", report.get("result"));
    line("restarts", report.get("restarts"));
    line("main status", report.get("exec_main_status"));
    line("stdout", report.get("standard_output"));
    line("stderr", report.get("standard_error"));
    line("log", report.get("log"));
    let tail = report
        .get("tail")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if tail.is_empty() {
        println!("\nthe runner has written no diagnostic log");
    } else {
        println!("\n{tail}");
    }
    Ok(())
}
