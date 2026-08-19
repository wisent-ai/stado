//! `stado disk-cleanup` / `stado install-disk-cleanup`.
//!
//! Port of the `disk_cleanup` and `install_disk_cleanup` commands from
//! `stado/cli.py`. The install side is the `kind == "disk-cleanup"` slice
//! of `stado/deploy/local_install.py` and now delegates to the shared
//! implementation in [`crate::deploy::local_install`] — faithful to
//! `install_local(SimpleNamespace(name="disk-cleanup"), "disk-cleanup",
//! False, click.echo)`, including the guard that never puts the HF write
//! token into the disk-cleanup unit env.
//!
//! DEVIATION from Python: `--dry-run` has no Python original. It runs
//! [`crate::providers::local::disk_cleanup::preview_cleanup_once`] instead
//! of `run_cleanup_once`, and exists so `stado host cleanup TARGET
//! --dry-run` has something to invoke on the host it is previewing —
//! see [`crate::deploy::host_cleanup`].

use std::time::Duration;

use serde_json::Value;

use super::CmdError;
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
        // The janitor's OWN planning phase: same canonical policy, same
        // lock, same scanners, with an `enforce` policy pinned to its
        // `report` mode and no state written. `stado host cleanup TARGET
        // --dry-run` (`deploy::host_cleanup`) runs exactly this over ssh
        // on the host being previewed.
        let report = disk_cleanup::preview_cleanup_once(&mut |_message| {}).await;
        println!("{}", disk_cleanup::canonical_json(&report));
        return Ok(());
    }
    loop {
        let report = disk_cleanup::run_cleanup_once(0, false, &mut |_message| {}).await;
        println!("{}", disk_cleanup::canonical_json(&report));
        if !watch {
            return Ok(());
        }
        let interval = report
            .get("check_interval_seconds")
            .and_then(Value::as_i64)
            .unwrap_or(60);
        tokio::time::sleep(Duration::from_secs(interval.max(60) as u64)).await;
    }
}

/// `install-disk-cleanup` command body (Python `install_disk_cleanup` →
/// `install_local(..., "disk-cleanup", False, click.echo)`; dry_run is
/// always false from this command).
pub async fn install() -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let hf_fetch = crate::deploy::local_install::production_hf_fetcher();
    let mut echo = |line: &str| println!("{line}");
    crate::deploy::local_install::install_local(
        "disk-cleanup",
        "disk-cleanup",
        false,
        &runner,
        &hf_fetch,
        &mut echo,
    )
    .await
    .map_err(|exc| CmdError::click(exc.to_string()))
}
