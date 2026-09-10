//! `stado workdirs`: preview or explicitly remove every scratch directory.

use crate::cli::CmdError;
use crate::providers::local::scratch_workdirs;

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

pub fn run(apply: bool, include_files: bool, json: bool) -> Result<(), CmdError> {
    let report = scratch_workdirs::sweep(apply, include_files);
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "scratch root {}: {} directory(ies), {:.1} GiB apparent size",
            report.root.display(),
            report.directories.len(),
            report.apparent_bytes as f64 / GIB
        );
        if !report.root_present && report.failed.is_empty() {
            println!("scratch root does not exist");
        }
        for entry in if apply {
            &report.removed
        } else {
            &report.directories
        } {
            println!(
                "  {} {} · {:.1} GiB apparent size",
                if apply { "removed" } else { "would remove" },
                entry.path.display(),
                entry.bytes as f64 / GIB
            );
        }
        for entry in &report.removed_files {
            println!(
                "  removed {} · {:.1} GiB apparent size · loose file",
                entry.path.display(),
                entry.bytes as f64 / GIB
            );
        }
        if include_files {
            println!(
                "  {} {} loose file(s) or link(s) · {:.1} GiB",
                if apply { "removed" } else { "would remove" },
                report.stray_files,
                report.bytes_stray_files as f64 / GIB
            );
        } else {
            println!(
                "  preserved {} loose file(s) or link(s) · {:.1} GiB",
                report.stray_files,
                report.bytes_stray_files as f64 / GIB
            );
        }
        for failure in &report.failed {
            eprintln!(
                "  failed {}: {}: {}",
                failure.path.display(),
                failure.operation,
                failure.error
            );
        }
        if apply {
            println!(
                "removed {} directory(ies); {} remain; {:.1} GiB apparent size removed",
                report.removed.len(),
                report.remaining_directories.len(),
                report.apparent_bytes_removed as f64 / GIB
            );
            if let (Some(before), Some(after)) = (report.free_bytes_before, report.free_bytes_after)
            {
                println!("filesystem free: {:.1} GiB before, {:.1} GiB after (not inferred from file sizes)",
                    before as f64 / GIB, after as f64 / GIB);
            }
            for path in &report.remaining_directories {
                println!("  remaining {}", path.display());
            }
        } else {
            println!("pass --apply to remove all directories, including jobs, runs and run-signals; active contents are not preserved. Add --include-files to empty the root of loose files and links as well");
        }
    }
    if !report.complete() {
        return Err(CmdError::click(format!(
            "working directory cleanup incomplete: {} failure(s), {} directory(ies) remain",
            report.failed.len(),
            report.remaining_directories.len()
        )));
    }
    Ok(())
}
