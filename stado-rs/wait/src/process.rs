//! Waits on a child process: run a command to its end, or collect a child
//! already started.

use std::fmt::Display;

use crate::{blocking, until, Kind};

/// The program and arguments `command` runs, as a person would type them.
fn command_line(command: &std::process::Command) -> String {
    std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|part| part.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Where `command` runs: its directory, or this process's own when none is
/// set.
fn directory(command: &std::process::Command) -> String {
    match command.get_current_dir() {
        Some(directory) => directory.display().to_string(),
        None => "the caller's directory".to_string(),
    }
}

/// Run `command` to its end and collect what it printed, saying so at both
/// ends: what is waited for is its command line, where is the directory it
/// runs in.
pub fn output(command: &mut std::process::Command) -> std::io::Result<std::process::Output> {
    let place = directory(command);
    blocking(Kind::Process, command_line(command), place, || {
        command.output()
    })
}

/// The same for a `tokio` command: run it to its end and collect what it
/// printed, saying so at both ends.
pub async fn output_async(
    command: &mut tokio::process::Command,
) -> std::io::Result<std::process::Output> {
    let standard = command.as_std();
    let place = directory(standard);
    let what = command_line(standard);
    until(Kind::Process, what, place, command.output()).await
}

/// Run `command` to its end with its output going where it was told to,
/// saying so at both ends.
pub fn status(command: &mut std::process::Command) -> std::io::Result<std::process::ExitStatus> {
    let place = directory(command);
    blocking(Kind::Process, command_line(command), place, || {
        command.status()
    })
}

/// The same for a `tokio` command.
pub async fn status_async(
    command: &mut tokio::process::Command,
) -> std::io::Result<std::process::ExitStatus> {
    let standard = command.as_std();
    let place = directory(standard);
    let what = command_line(standard);
    until(Kind::Process, what, place, command.status()).await
}

/// Wait for a child already started — `what` names it, as its caller knows
/// it — and collect what it printed, saying so at both ends.
pub fn child_output(
    child: std::process::Child,
    what: impl Display,
) -> std::io::Result<std::process::Output> {
    let place = format!("pid {}", child.id());
    blocking(Kind::Process, what, place, || child.wait_with_output())
}

/// The same for a `tokio` child already started: `what` names it, as its
/// caller knows it.
pub async fn child_output_async(
    child: tokio::process::Child,
    what: impl Display,
) -> std::io::Result<std::process::Output> {
    let place = match child.id() {
        Some(pid) => format!("pid {pid}"),
        None => "a child that has already ended".to_string(),
    };
    until(Kind::Process, what, place, child.wait_with_output()).await
}
