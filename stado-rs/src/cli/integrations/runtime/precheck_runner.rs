//! The GitHub pre-check runner as a role of the host's one Stado process.
//!
//! A runner host used to carry a second unit of its own, a system daemon
//! whose only program was the runner's launcher. Stado is one process per
//! host, so the launcher is started here instead: `stado serve
//! --precheck-runner <ROOT>` runs `<ROOT>/start-runner.sh` through
//! passwordless sudo, the same grant every step of the runner installer
//! already runs under. The launcher applies the runner's egress rules and
//! drops to the runner's own service account before it executes GitHub's
//! listener; this process never holds the runner's identity itself.
//!
//! The listener's exit is this role's failure, and the supervisor ends the
//! whole process on it, so the init system restarts `com.wisent.stado` and
//! the listener with it rather than keeping a host that silently stopped
//! taking jobs.

use std::path::{Path, PathBuf};

use crate::cli::CmdError;

/// The launcher the runner installer writes into the runner root.
pub(crate) const LAUNCHER: &str = "start-runner.sh";

/// The one way this process reaches root: the grant the installer requires.
const SUDO: &str = "/usr/bin/sudo";

pub(crate) async fn run(root: PathBuf) -> Result<(), CmdError> {
    let launcher = root.join(LAUNCHER);
    if !launcher.is_file() {
        return Err(CmdError::click(format!(
            "the pre-check runner launcher {} does not exist; `stado runner install` writes it",
            launcher.display()
        ))
        .stating(crate::primitives::failure::FailureCode::NotFound));
    }
    let mut child = tokio::process::Command::new(SUDO)
        .arg("-n")
        .arg("--")
        .arg(&launcher)
        .current_dir(&root)
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| {
            CmdError::click(format!(
                "the pre-check runner launcher {} could not start: {error}",
                launcher.display()
            ))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
    // The pid is read once: a child that exited before the read has none,
    // and both the log line and the wait description say so.
    let pid = match child.id() {
        Some(pid) => format!("pid {pid}"),
        None => "an exited child".to_string(),
    };
    eprintln!(
        "[stado serve precheck-runner] {} ({pid})",
        launcher.display()
    );
    let status = crate::wait::until(
        crate::wait::Kind::Process,
        format!("the pre-check runner launcher {}", launcher.display()),
        pid,
        child.wait(),
    )
    .await
    .map_err(|error| {
        CmdError::click(format!(
            "the pre-check runner launcher {} could not be waited on: {error}",
            launcher.display()
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    Err(CmdError::click(format!(
        "the pre-check runner launcher {} exited ({status}); {} takes no job until \
         com.wisent.stado restarts",
        launcher.display(),
        runner_name(&root)
    ))
    .stating(crate::primitives::failure::FailureCode::InfraDown))
}

/// The runner root's own name, for a message that has to say which runner
/// stopped on a host that carries more than one.
fn runner_name(root: &Path) -> String {
    // A root such as `/` has no final component; the whole path names it.
    match root.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => root.display().to_string(),
    }
}
