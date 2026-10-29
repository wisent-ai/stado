//! `stado host processes TARGET [--json]`: what each process on TARGET spent
//! since the reading before, read by the host's own Stado from the kernel
//! (`crate::providers::local::host_processes`).
//!
//! `top` and `ps` are other programs, and an agent session, a sandbox or the
//! host-exec allowlist can be refused them; this reading needs only the Stado
//! process itself. On another host the command runs that host's installed
//! Stado over the host channel, which reads its own kernel the same way.

use serde_json::Value;

use crate::cli::host::checks::probes::{cell, print_json};
use crate::cli::CmdError;

pub async fn processes(target: &str, json: bool) -> Result<(), CmdError> {
    let resolved = crate::cli::canonical_host(target).await?;
    let report: Value = if crate::deploy::host_channel::target_is_this_host(&resolved) {
        let report =
            crate::providers::local::host_processes::read_and_compare().map_err(|cause| {
                CmdError::unreachable(format!(
                    "{}: the processes could not be read: {cause}",
                    resolved.name
                ))
            })?;
        serde_json::to_value(report)?
    } else {
        let arguments = ["host", "processes", resolved.name.as_str(), "--json"];
        let answered = crate::cli::host::remote_stado_output(&resolved.name, &arguments).await?;
        serde_json::from_str(&answered).map_err(|error| {
            CmdError::unreachable(format!(
                "{}: its Stado answered host processes with something that is not its report: \
                 {error}: {}",
                resolved.name,
                answered.trim()
            ))
        })?
    };
    if json {
        print_json(&report);
        return Ok(());
    }
    render(&resolved.name, &report);
    Ok(())
}

fn render(host: &str, report: &Value) {
    let count = |key: &str| match report.get(key).and_then(Value::as_array) {
        Some(items) => items.len().to_string(),
        None => crate::cli::reporting::NOT_REPORTED.to_string(),
    };
    if report.get("state").and_then(Value::as_str) == Some("first_reading") {
        println!(
            "{host}: first reading, {} process(es) read; the next run reports what each spent \
             since this one",
            cell(report.get("read"))
        );
    } else {
        println!(
            "{host}: over {} s, {} of {} process(es) spent CPU or moved bytes; {} ended in between",
            cell(report.get("interval_seconds")),
            count("processes"),
            cell(report.get("read")),
            count("ended")
        );
        let rows: Vec<Vec<String>> = report
            .get("processes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|process| {
                [
                    "pid",
                    "cpu_seconds",
                    "children_cpu_seconds",
                    "resident_bytes",
                    "disk_read_bytes",
                    "disk_written_bytes",
                    "program",
                ]
                .iter()
                .map(|field| cell(process.get(*field)))
                .collect()
            })
            .collect();
        crate::cli::reporting::table::print(
            &[
                "PID",
                "CPU_S",
                "CHILDREN_CPU_S",
                "RSS_BYTES",
                "READ_BYTES",
                "WRITTEN_BYTES",
                "PROGRAM",
            ],
            &rows,
        );
    }
    for refused in report
        .get("unreadable")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        println!(
            "unreadable: pid {}: {}",
            cell(refused.get("pid")),
            cell(refused.get("error"))
        );
    }
}
