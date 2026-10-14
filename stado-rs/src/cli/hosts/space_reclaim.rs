//! Local `stado space reclaim --local` execution and the host health watch.
//!
//! Reclamation applies the disk-full rule on this machine. A preview uses
//! the same lock and scanners but removes nothing. The standing watch reads
//! the volume at the host's declared health cadence.

use std::num::NonZeroU64;
use std::time::Duration;

use crate::cli::CmdError;
use crate::providers::local::space_reclaim;

/// Local reclamation command body: one pass, or one preview.
pub async fn run(dry_run: bool) -> Result<(), CmdError> {
    if dry_run {
        // The janitor's OWN planning phase: same lock, same scanners, nothing
        // removed and no state written. The `registry_cleanup` stage
        // (`deploy::host_state::cleanup`) runs exactly this on the target
        // being previewed.
        let report = space_reclaim::preview_cleanup_once(&mut |_message| {}).await;
        println!("{}", space_reclaim::canonical_json(&report));
        return Ok(());
    }
    let writer = space_reclaim::CleanupWriter::Cli { every: None };
    let report = space_reclaim::run_cleanup_once(0, writer, &mut |_message| {}).await;
    println!("{}", space_reclaim::canonical_json(&report));
    Ok(())
}

/// The host's standing reclaim watch: one threshold-governed pass each cadence.
pub async fn watch(every: NonZeroU64) -> Result<(), CmdError> {
    let every = Duration::from_secs(every.get());
    let writer = space_reclaim::CleanupWriter::Cli { every: Some(every) };
    loop {
        let report = space_reclaim::run_cleanup_once(0, writer, &mut |_message| {}).await;
        println!("{}", space_reclaim::canonical_json(&report));
        tokio::time::sleep(every).await;
    }
}
