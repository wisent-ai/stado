//! Product sync, the host process's delivery component: it installs the
//! release that replaces the one running.
//!
//! It is started before every other role and outlives a stopped one
//! ([`super::supervisor::Supervisor::spawn_delivery`]): a release whose
//! resolver refuses to start stopped the whole process within seconds of each
//! restart, product sync with it, so the host could never install the repair
//! of the release it was running. When a pass replaces the program this
//! process runs, the component ends, and with it the service, so the init
//! system starts the installed release instead of this one living on.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::cli::CmdError;

/// `stado product`, as the CLI defines it.
fn product_command() -> clap::Command {
    stado_product::cli::augment(clap::Command::new("product"))
}

/// The workspace the host process's product sync reads and builds from: a
/// checkout of each product's origin that Stado owns, beside the products'
/// other state, never the operator's working checkouts. Those sit under
/// ~/Documents, which macOS lets a program read only after the operator allows
/// it in System Settings, and they carry uncommitted work that holds a product;
/// a clone Stado owns needs neither.
fn product_sync_workspace() -> PathBuf {
    crate::config_file::expand_tilde("~/.stado/products/workspace")
}

/// What identifies the program file: its length and modification time. A
/// release installed over it changes both.
fn program_identity(program: &Path) -> Result<(u64, SystemTime), CmdError> {
    let metadata = std::fs::metadata(program).map_err(|error| {
        CmdError::click(format!(
            "product sync cannot read the program this process runs, {}: {error}",
            program.display()
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    let modified = metadata.modified().map_err(|error| {
        CmdError::click(format!(
            "product sync cannot read when {} changed: {error}",
            program.display()
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    Ok((metadata.len(), modified))
}

/// One `stado product sync` pass over `surface`, reported on stderr.
async fn sync_pass(workspace: String, surface: String) {
    let line = vec![
        "product".to_string(),
        "--workspace".to_string(),
        workspace,
        "sync".to_string(),
        "--surface".to_string(),
        surface.clone(),
        "--fetch".to_string(),
        "--clone-missing".to_string(),
    ];
    let outcome = tokio::task::spawn_blocking(move || {
        let matches = product_command()
            .try_get_matches_from(line)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        stado_product::cli::run(matches, crate::cli::setup::product::build())
    })
    .await;
    match outcome {
        Ok(Ok(status)) if status == nix::libc::EXIT_SUCCESS => {}
        Ok(Ok(status)) => {
            eprintln!("[stado serve product-sync] {surface} sync exited with status {status}")
        }
        Ok(Err(error)) => {
            eprintln!("[stado serve product-sync] {surface} sync failed: {error:#}")
        }
        Err(error) => eprintln!("[stado serve product-sync] {surface} sync stopped: {error}"),
    }
}

pub(super) async fn product_sync(period: Duration, surfaces: Vec<String>) -> Result<(), CmdError> {
    let program = std::env::current_exe().map_err(|error| {
        CmdError::unreachable(format!(
            "product sync cannot name the program this process runs: {error}"
        ))
    })?;
    let started_as = program_identity(&program)?;
    let mut schedule = tokio::time::interval(period);
    schedule.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let workspace = product_sync_workspace().to_string_lossy().into_owned();
    loop {
        schedule.tick().await;
        for surface in &surfaces {
            sync_pass(workspace.clone(), surface.clone()).await;
        }
        if program_identity(&program)? != started_as {
            return Err(CmdError::click(format!(
                "product sync installed a new release over {}, the program this process runs; \
                 the service ends so its init system starts the installed release",
                program.display()
            )));
        }
    }
}
