//! `stado waits measured`: every operation whose waits completed on this
//! machine, with the longest completed wait on it, longest first.
//!
//! Each wait already records how long it took under its operation, and the
//! next wait on that operation is due by that measurement (`stado-wait`'s
//! `measure`). This is the reading of those records: which builds, channels,
//! child processes and requests take longest here, so a slow operation is
//! seen in one place and its repair shows as a shorter measurement. `--over`
//! is the reader's own threshold; no number here is Stado's.

use serde_json::json;

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

pub(super) fn list(over: Option<u64>, as_json: bool) -> Result<(), CmdError> {
    let directory = crate::wait::measured::directory().map_err(|cause| {
        CmdError::click(format!("the measured waits cannot be found: {cause}"))
            .stating(FailureCode::Config)
    })?;
    let kept = crate::wait::measured::kept().map_err(|cause| {
        CmdError::click(format!("the measured waits cannot be read: {cause}"))
            .stating(FailureCode::Config)
    })?;
    let shown: Vec<_> = kept
        .into_iter()
        .filter(|(_, took)| over.is_none_or(|seconds| took.as_secs() > seconds))
        .collect();
    if as_json {
        let rows: Vec<_> = shown
            .iter()
            .map(|(operation, took)| {
                json!({"operation": operation, "longest_seconds": took.as_secs_f64()})
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "directory": directory.display().to_string(),
                "over_seconds": over,
                "measured": rows,
            }))?
        );
        return Ok(());
    }
    println!(
        "measured waits {}: {} operation(s)",
        directory.display(),
        shown.len()
    );
    for (operation, took) in &shown {
        println!("  {:>10.3}s  {operation}", took.as_secs_f64());
    }
    Ok(())
}
