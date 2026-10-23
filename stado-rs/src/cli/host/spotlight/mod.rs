//! `stado host spotlight TARGET [--apply] [--json]`: which build caches in
//! TARGET's home Spotlight still indexes, and with `--apply` the marker that
//! keeps it out of each (`crate::deploy::host_spotlight`).

use serde_json::{json, Value};

use crate::cli::CmdError;

/// Report, or with `apply` mark, the tagged build caches of `target`. The
/// command fails when the walk failed or any marker could not be written,
/// naming each; a report that leaves caches unmarked succeeds and lists them.
pub async fn spotlight(target: &str, apply: bool, json_output: bool) -> Result<(), CmdError> {
    let target = crate::cli::canonical_host(target).await?;
    let runner = crate::deploy::production_runner();
    let report = crate::deploy::host_spotlight::exclude_on_host(&target, apply, &runner).await;
    let failed: Vec<&crate::deploy::host_spotlight::SpotlightEntry> = report
        .entries
        .iter()
        .filter(|entry| entry.state == "mark-failed" || entry.state == "unreadable")
        .collect();
    if json_output {
        let entries: Vec<Value> = report
            .entries
            .iter()
            .map(|entry| json!({ "state": entry.state, "path": entry.path, "cause": entry.cause }))
            .collect();
        println!(
            "{}",
            json!({ "target": report.target, "apply": apply, "entries": entries, "error": report.error })
        );
    } else {
        for entry in &report.entries {
            match &entry.cause {
                Some(cause) => println!("{}\t{}\t{cause}", entry.state, entry.path),
                None => println!("{}\t{}", entry.state, entry.path),
            }
        }
        if report.entries.is_empty() && report.error.is_none() {
            println!("{}: no tagged build cache in its home", report.target);
        }
    }
    if let Some(error) = report.error {
        return Err(CmdError::unreachable(format!(
            "host spotlight {}: {error}",
            report.target
        )));
    }
    if !failed.is_empty() {
        let named: Vec<String> = failed
            .iter()
            .map(|entry| match &entry.cause {
                Some(cause) => format!("{} ({cause})", entry.path),
                None => entry.path.clone(),
            })
            .collect();
        return Err(CmdError::click(format!(
            "host spotlight {}: no marker could be written in {}",
            report.target,
            named.join(", ")
        )));
    }
    Ok(())
}
