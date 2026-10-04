//! `stado disk-cleanup`.
//!
//! `disk-cleanup` applies the disk-full rule
//! ([`crate::providers::local::disk_cleanup::rule`]) on this machine. The
//! standing cleanup watch is the `--disk-cleanup` role of com.wisent.stado,
//! the one Stado process on a host.
//!
//! `--dry-run` runs
//! [`crate::providers::local::disk_cleanup::preview_cleanup_once`] instead
//! of `run_cleanup_once`, and is the target-local primitive used by the
//! `registry_cleanup` stage of `stado space reclaim`.

use std::time::Duration;

use crate::cli::CmdError;
use crate::providers::local::disk_cleanup;

/// `disk-cleanup` command body (Python `disk_cleanup`).
pub async fn run(once: bool, watch: bool, dry_run: bool) -> Result<(), CmdError> {
    if once && watch {
        return Err(CmdError::usage("--once and --watch are mutually exclusive"));
    }
    if dry_run && watch {
        // A preview is a single planning pass; there is nothing for a
        // watch loop to observe, and repeating it would just take the
        // exclusive cleanup lock over and over.
        return Err(CmdError::usage(
            "--dry-run and --watch are mutually exclusive",
        ));
    }
    if dry_run {
        // The janitor's OWN planning phase: same lock, same scanners, nothing
        // removed and no state written. The `registry_cleanup` stage
        // (`deploy::host_state::cleanup`) runs exactly this on the target
        // being previewed.
        let report = disk_cleanup::preview_cleanup_once(&mut |_message| {}).await;
        println!("{}", disk_cleanup::canonical_json(&report));
        return Ok(());
    }
    loop {
        let report =
            disk_cleanup::run_cleanup_once(0, disk_cleanup::CleanupWriter::Cli, &mut |_message| {})
                .await;
        println!("{}", disk_cleanup::canonical_json(&report));
        if !watch {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(disk_cleanup::rule::CHECK_SECONDS)).await;
    }
}
