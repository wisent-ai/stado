//! `stado product locks [--json]`: every product surface lock with its
//! recorded owner, process identity, observed state and corruption.

use anyhow::Context;

pub fn report_locks(runtime: &crate::common::Runtime, as_json: bool) -> anyhow::Result<i32> {
    let report = crate::common::files::locks_report(&runtime.home)?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for row in report["locks"]
            .as_array()
            .context("lock report has no rows")?
        {
            let path = row["path"].as_str().context("lock row has no path")?;
            if let Some(error) = row["error"].as_str() {
                println!("{path}: unreadable or corrupt: {error}");
                continue;
            }
            let state = match (row["held"].as_bool(), row["running"].as_bool()) {
                (Some(true), Some(true)) => "held",
                (Some(true), _) => "held with unverified process identity",
                (_, Some(true)) => "released record",
                _ => "stale record",
            };
            println!(
                "{path}: {state}; pid {}, born {}, observed {}; phase {}, step {}; acquired {}",
                row["pid"],
                row["started_at"],
                row["observed_start"],
                row["phase"],
                row["step"],
                row["acquired_at"]
            );
        }
    }
    Ok(libc::EXIT_SUCCESS)
}
