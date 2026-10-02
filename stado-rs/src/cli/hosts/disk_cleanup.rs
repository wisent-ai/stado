//! `stado disk-cleanup`.
//!
//! `disk-cleanup` runs the registry-declared cleanup pass on this machine.
//! The standing cleanup watch is the `--disk-cleanup` role of
//! com.wisent.stado, the one Stado process on a host.
//!
//! `--dry-run` runs
//! [`crate::providers::local::disk_cleanup::preview_cleanup_once`] instead
//! of `run_cleanup_once`, and is the target-local primitive used by the
//! `registry_cleanup` stage of `stado space reclaim`.

use std::time::Duration;

use serde_json::Value;

use crate::cli::CmdError;
use crate::providers::local::disk_cleanup;
use crate::providers::local::host_memory;

/// `disk-cleanup` command body (Python `disk_cleanup`).
pub async fn run(once: bool, watch: bool, to_target: bool, dry_run: bool) -> Result<(), CmdError> {
    if once && watch {
        return Err(CmdError::usage("--once and --watch are mutually exclusive"));
    }
    if to_target && (watch || dry_run) {
        return Err(CmdError::usage(
            "--to-target requires one enforcing pass and cannot be combined with --watch or --dry-run",
        ));
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
        // The janitor's OWN planning phase: same canonical policy, same
        // lock, same scanners, with an `enforce` policy pinned to its
        // `report` mode and no state written. The `registry_cleanup` stage
        // (`deploy::host_state::cleanup`) runs exactly this on the target being
        // previewed.
        let report = disk_cleanup::preview_cleanup_once(&mut |_message| {}).await;
        println!("{}", disk_cleanup::canonical_json(&report));
        return Ok(());
    }
    loop {
        let report = if to_target {
            disk_cleanup::run_cleanup_to_target_once(
                0,
                disk_cleanup::CleanupWriter::Cli,
                &mut |_message| {},
            )
            .await
        } else {
            disk_cleanup::run_cleanup_once(
                0,
                false,
                disk_cleanup::CleanupWriter::Cli,
                &mut |_message| {},
            )
            .await
        };
        println!("{}", disk_cleanup::canonical_json(&report));
        // The host's second declared janitor pass, on the same writer and in
        // the same loop. `targets[].disk_cleanup` and
        // `targets[].memory_reclaim` are two declarations executed by the same
        // two writers — this unit on its timer, and the queue agent's janitor
        // task on every tick — so that neither needs an autonomy mode change
        // or an operator gesture to reach a host.
        let memory = host_memory::run_memory_pass_once(
            0,
            host_memory::MemoryWriter::Cli,
            &mut |_message| {},
        )
        .await;
        println!("{}", disk_cleanup::canonical_json(&memory));
        if !watch {
            return Ok(());
        }
        // The registry's `check_interval_seconds` is the cadence. A pass that
        // could not read its policy — the store answered 502 for one tick —
        // reports none, and the watch then reads the cadence from the
        // declaration itself, through the last-known-good registry copy; the
        // pass's own failure is already in its report. Only a target that
        // declares no cleanup at all ends the watch with that fact.
        let interval = match report.get("check_interval_seconds").and_then(Value::as_u64) {
            Some(interval) => interval,
            None => declared_cadence().await?,
        };
        tokio::time::sleep(Duration::from_secs(interval)).await;
    }
}

/// This host's declared cleanup cadence, from the registry target.
async fn declared_cadence() -> Result<u64, CmdError> {
    let hostname = crate::providers::vast::system_hostname();
    let target = crate::providers::local::agent::lookup_self_auto(&hostname)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
        .ok_or_else(|| {
            CmdError::click(format!(
                "{hostname} is not a registry target, so --watch has no declared cadence to run on"
            ))
        })?;
    let declared = target.disk_cleanup.as_ref().ok_or_else(|| {
        CmdError::click(format!(
            "{} declares no disk_cleanup, so --watch has no declared cadence to run on",
            target.name
        ))
    })?;
    u64::try_from(declared.check_interval_seconds).map_err(|_| {
        CmdError::click(format!(
            "{} declares check_interval_seconds {}, which is not a cadence",
            target.name, declared.check_interval_seconds
        ))
    })
}
