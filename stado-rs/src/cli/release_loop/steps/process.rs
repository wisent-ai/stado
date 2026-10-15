//! The programs the delivery loop runs — Oko's CLI, git and cargo in a
//! product checkout — each resolved to the path this machine installs it at
//! and waited on through Stado's wait lines, so what the loop stands on is
//! written before it stands there.

use std::path::{Path, PathBuf};

use tokio::process::Command;

use crate::cli::CmdError;

/// `program` at the path the fleet's program table declares for it
/// (`installed_program`): a service unit's `PATH` holds none of
/// `~/.stado/bin`, `~/.cargo/bin` or Homebrew's, so a bare name there is not
/// found. A program the table does not declare is run as named.
fn resolved(program: &str) -> Result<PathBuf, CmdError> {
    crate::deploy::host_exec::installed_program(program)
        .map_err(|missing| CmdError::click(format!("{program} cannot run: {missing}")))
}

/// `program args` run in `directory` (the caller's when none) to its end:
/// its standard output on success, else a refusal naming the command, its
/// exit and its standard error.
pub async fn run(
    program: &str,
    args: &[&str],
    directory: Option<&Path>,
) -> Result<String, CmdError> {
    let program = resolved(program)?;
    let shown = format!("{} {}", program.display(), args.join(" "));
    let place = match directory {
        Some(directory) => directory.display().to_string(),
        None => "the caller's directory".to_string(),
    };
    let mut command = Command::new(&program);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    if let Some(directory) = directory {
        command.current_dir(directory);
    }
    let output = crate::wait::until(crate::wait::Kind::Process, &shown, place, command.output())
        .await
        .map_err(|error| CmdError::click(format!("{shown} did not run: {error}")))?;
    if !output.status.success() {
        return Err(CmdError::click(format!(
            "{shown} exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Whether `program args` in `directory` exits zero; a program that cannot
/// be started is a refusal, not a no.
pub async fn succeeds(program: &str, args: &[&str], directory: &Path) -> Result<bool, CmdError> {
    let program = resolved(program)?;
    let shown = format!("{} {}", program.display(), args.join(" "));
    let mut command = Command::new(&program);
    command
        .args(args)
        .current_dir(directory)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let output = crate::wait::until(
        crate::wait::Kind::Process,
        &shown,
        directory.display(),
        command.output(),
    )
    .await
    .map_err(|error| CmdError::click(format!("{shown} did not run: {error}")))?;
    Ok(output.status.success())
}
