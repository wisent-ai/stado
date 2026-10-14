//! The programs the delivery loop runs — Oko's CLI, git and cargo in a
//! product checkout — each waited on through Stado's wait lines, so what the
//! loop stands on is written before it stands there.

use std::path::Path;

use tokio::process::Command;

use crate::cli::CmdError;

/// `program args` run in `directory` (the caller's when none) to its end:
/// its standard output on success, else a refusal naming the command, its
/// exit and its standard error.
pub async fn run(
    program: &str,
    args: &[&str],
    directory: Option<&Path>,
) -> Result<String, CmdError> {
    let shown = format!("{program} {}", args.join(" "));
    let place = directory.map_or_else(
        || "the caller's directory".to_string(),
        |directory| directory.display().to_string(),
    );
    let mut command = Command::new(program);
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
    let shown = format!("{program} {}", args.join(" "));
    let mut command = Command::new(program);
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
