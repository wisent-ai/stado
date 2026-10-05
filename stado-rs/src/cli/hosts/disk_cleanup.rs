//! `stado disk-cleanup` and the `--disk-cleanup` role of `stado serve`.
//!
//! `disk-cleanup` applies the disk-full rule
//! ([`crate::providers::local::disk_cleanup::rule`]) on this machine once.
//! The standing cleanup watch is the `--disk-cleanup` role of
//! com.wisent.stado, the one Stado process on a host, reading the volume at
//! the host's declared health cadence.
//!
//! `--dry-run` runs
//! [`crate::providers::local::disk_cleanup::preview_cleanup_once`] instead
//! of `run_cleanup_once`, and is the target-local primitive used by the
//! `registry_cleanup` stage of `stado space reclaim`.

use std::num::NonZeroU64;
use std::time::Duration;

use crate::cli::CmdError;
use crate::providers::local::disk_cleanup;

/// `disk-cleanup` command body: one pass, or one preview.
pub async fn run(dry_run: bool) -> Result<(), CmdError> {
    if dry_run {
        // The janitor's OWN planning phase: same lock, same scanners, nothing
        // removed and no state written. The `registry_cleanup` stage
        // (`deploy::host_state::cleanup`) runs exactly this on the target
        // being previewed.
        let report = disk_cleanup::preview_cleanup_once(&mut |_message| {}).await;
        println!("{}", disk_cleanup::canonical_json(&report));
        return Ok(());
    }
    let writer = disk_cleanup::CleanupWriter::Cli { every: None };
    let report = disk_cleanup::run_cleanup_once(0, writer, &mut |_message| {}).await;
    println!("{}", disk_cleanup::canonical_json(&report));
    Ok(())
}

/// The `--disk-cleanup` role: a pass every `every`, the period the watch
/// reads the volume at and promises its next pass by.
pub async fn watch(every: NonZeroU64) -> Result<(), CmdError> {
    let every = Duration::from_secs(every.get());
    let writer = disk_cleanup::CleanupWriter::Cli { every: Some(every) };
    loop {
        let report = disk_cleanup::run_cleanup_once(0, writer, &mut |_message| {}).await;
        println!("{}", disk_cleanup::canonical_json(&report));
        tokio::time::sleep(every).await;
    }
}
