//! Local `stado space reclaim --local` execution and the host health watch.
//!
//! Reclamation applies the disk-full rule on this machine. A preview uses
//! the same lock and scanners but removes nothing. The standing watch reads
//! the volume at the host's declared health cadence.

use std::num::NonZeroU64;
use std::time::Duration;

use crate::cli::CmdError;
use crate::providers::local::space_reclaim;

/// The workloads holding the janitor's lock on this host now, every job the
/// agent runs among them: the running-job count a pass made outside the agent
/// reports, so its cleaners skip the caches a running job reads. A pass used
/// to report none, and cleared the model cache under a running job.
fn running_workloads() -> Result<i64, CmdError> {
    space_reclaim::live_workload_count().map_err(|error| {
        CmdError::click(format!(
            "the workloads running on this host cannot be counted, so no reclaim pass runs: \
             {error}"
        ))
    })
}

/// Local reclamation command body: one pass, or one preview.
pub async fn run(dry_run: bool) -> Result<(), CmdError> {
    if dry_run {
        // The janitor's OWN planning phase: same lock, same scanners, nothing
        // removed and no state written. The `registry_cleanup` stage
        // (`deploy::host_state::cleanup`) runs exactly this on the target
        // being previewed.
        let report =
            space_reclaim::preview_cleanup_once(running_workloads()?, &mut |_message| {}).await;
        println!("{}", space_reclaim::canonical_json(&report));
        return Ok(());
    }
    let writer = space_reclaim::CleanupWriter::Cli { every: None };
    let report =
        space_reclaim::run_cleanup_once(running_workloads()?, writer, &mut |_message| {}).await;
    println!("{}", space_reclaim::canonical_json(&report));
    Ok(())
}

/// The host's standing reclaim watch: one threshold-governed pass each
/// cadence, a pass that outlasts the cadence starting the next at once and
/// skipping the ones it missed. A cadence whose running workloads cannot be
/// counted runs no pass and says why; the next cadence counts again.
pub async fn watch(every: NonZeroU64) -> Result<(), CmdError> {
    let every = Duration::from_secs(every.get());
    let writer = space_reclaim::CleanupWriter::Cli { every: Some(every) };
    let mut schedule = tokio::time::interval(every);
    schedule.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        schedule.tick().await;
        match running_workloads() {
            Ok(running) => {
                let report =
                    space_reclaim::run_cleanup_once(running, writer, &mut |_message| {}).await;
                println!("{}", space_reclaim::canonical_json(&report));
            }
            Err(refusal) => eprintln!("{refusal}"),
        }
    }
}
