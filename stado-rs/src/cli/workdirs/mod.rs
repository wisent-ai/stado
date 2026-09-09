//! `stado workdirs`: what the scratch root holds, and its removal.
//!
//! Reporting is the default and removal needs `--apply`, the same shape the
//! rest of the fleet's destructive verbs use, so a plan can be read before a
//! disk changes.

use crate::cli::CmdError;
use crate::providers::local::scratch_workdirs;

/// Bytes per gibibyte, for the operator-facing line only. The report itself
/// carries exact byte counts.
const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// `workdirs` command body.
pub fn run(apply: bool, json: bool) -> Result<(), CmdError> {
    let report = scratch_workdirs::sweep(apply);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|error| CmdError::click(error.to_string()))?
        );
        return Ok(());
    }
    if !report.root_present {
        println!(
            "{} does not exist, so this host holds no scratch working directories",
            report.root.display()
        );
        return Ok(());
    }
    println!(
        "scratch root {}: {} undeclared working directory(ies), {:.1} GiB",
        report.root.display(),
        report.undeclared.len(),
        report.bytes_undeclared as f64 / GIB
    );
    for area in &report.declared {
        println!(
            "  left alone {} · {:.1} GiB · {}",
            area.name,
            area.bytes as f64 / GIB,
            area.owner
        );
    }
    if report.stray_files > 0 {
        println!(
            "  left alone {} loose file(s) at the root · {:.1} GiB · not working directories",
            report.stray_files,
            report.bytes_stray_files as f64 / GIB
        );
    }
    if !apply {
        for entry in &report.undeclared {
            println!(
                "  would remove {} · {:.1} GiB",
                entry.path.display(),
                entry.bytes as f64 / GIB
            );
        }
        println!("pass --apply to remove them");
        return Ok(());
    }
    for removal in &report.removed {
        println!(
            "  removed {} · {:.1} GiB",
            removal.path.display(),
            removal.bytes as f64 / GIB
        );
    }
    for removal in &report.failed {
        println!(
            "  failed {} · {}",
            removal.path.display(),
            removal.error.as_deref().unwrap_or("unknown error")
        );
    }
    println!(
        "removed {} of {} directory(ies), {:.1} GiB reclaimed",
        report.removed.len(),
        report.undeclared.len(),
        report.bytes_removed as f64 / GIB
    );
    if !report.failed.is_empty() {
        return Err(CmdError::click(format!(
            "{} working directory(ies) could not be removed",
            report.failed.len()
        )));
    }
    Ok(())
}
