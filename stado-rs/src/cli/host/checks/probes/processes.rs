//! `stado host processes TARGET [--within SECONDS] [--json]`: what each
//! process on TARGET spent since the reading before, read by the host's own
//! Stado from the kernel (`crate::providers::local::host_processes`), and
//! which processes grow fast enough to leave it no memory before the next
//! reading.
//!
//! `top` and `ps` are other programs, and an agent session, a sandbox or the
//! host-exec allowlist can be refused them; this reading needs only the Stado
//! process itself. On another host the command runs that host's installed
//! Stado over the host channel, which reads its own kernel the same way.

use serde_json::Value;

use crate::cli::host::checks::probes::{cell, print_json};
use crate::cli::CmdError;

pub async fn processes(
    target: &str,
    within: Option<std::num::NonZeroU64>,
    json: bool,
) -> Result<(), CmdError> {
    use crate::providers::local::host_processes::{read_and_compare_within, Horizon};
    let resolved = crate::cli::canonical_host(target).await?;
    let report: Value = if crate::deploy::host_channel::target_is_this_host(&resolved) {
        let horizon = match within {
            Some(seconds) => Horizon::Within(std::time::Duration::from_secs(seconds.get())),
            None => Horizon::Measured,
        };
        let report = read_and_compare_within(horizon).map_err(|cause| {
            CmdError::unreachable(format!(
                "{}: the processes could not be read: {cause}",
                resolved.name
            ))
        })?;
        serde_json::to_value(report)?
    } else {
        let within = within.map(|seconds| seconds.to_string());
        let mut arguments = vec!["host", "processes", resolved.name.as_str(), "--json"];
        if let Some(seconds) = within.as_deref() {
            arguments.extend(["--within", seconds]);
        }
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
        if let Some(cause) = report.get("not_compared").and_then(Value::as_str) {
            println!("not compared with the kept reading: {cause}");
        }
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
                    "resident_change_bytes",
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
                "RSS_CHANGE_BYTES",
                "READ_BYTES",
                "WRITTEN_BYTES",
                "PROGRAM",
            ],
            &rows,
        );
        render_memory(report.get("memory"));
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

/// The memory verdict: its one line, then for a runaway one line per named
/// process with what it grew and how soon it alone uses up the memory.
fn render_memory(memory: Option<&Value>) {
    let Some(memory) = memory.filter(|memory| !memory.is_null()) else {
        println!("memory: {}", crate::cli::reporting::NOT_REPORTED);
        return;
    };
    println!("memory: {}", memory_line(memory));
    if memory
        .get("available_unread")
        .is_some_and(|cause| !cause.is_null())
    {
        return;
    }
    for process in memory
        .get("processes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        println!(
            "  pid {} grew {} byte(s) to {}; alone it uses up the available memory in {} s: {}",
            cell(process.get("pid")),
            cell(process.get("grown_bytes")),
            cell(process.get("resident_bytes")),
            cell(process.get("exhausts_available_in_seconds")),
            cell(process.get("program"))
        );
    }
}

/// A published memory verdict as one line, which `stado host processes` and
/// `stado host link` both print: the processes named and when they use up
/// the available memory, or how long all the growth leaves, or why nothing
/// was judged.
pub(in crate::cli::host) fn memory_line(memory: &Value) -> String {
    let horizon = format!(
        "the next reading in {} s ({})",
        cell(memory.get("horizon_seconds")),
        cell(memory.get("horizon_source"))
    );
    if memory
        .get("available_unread")
        .is_some_and(|cause| !cause.is_null())
    {
        return format!(
            "not judged, since the available memory was not read: {}",
            cell(memory.get("available_unread"))
        );
    }
    let named: Vec<String> = memory
        .get("processes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|process| {
            format!(
                "pid {} ({})",
                cell(process.get("pid")),
                cell(process.get("program"))
            )
        })
        .collect();
    if !named.is_empty() {
        return format!(
            "RUNAWAY: {} use up the {} available byte(s) in {} s at this interval's growth, \
             before {horizon}",
            named.join(", "),
            cell(memory.get("available_bytes")),
            cell(memory.get("exhausts_available_in_seconds"))
        );
    }
    match memory
        .get("exhausts_available_in_seconds")
        .filter(|seconds| !seconds.is_null())
    {
        Some(seconds) => format!(
            "{} byte(s) available; all growth of the interval together ({} byte(s)) would use \
             it up in {} s, past {horizon}",
            cell(memory.get("available_bytes")),
            cell(memory.get("grown_bytes")),
            cell(Some(seconds))
        ),
        None => format!(
            "{} byte(s) available; no process grew in the interval",
            cell(memory.get("available_bytes"))
        ),
    }
}
