//! Waits on a child process: run a command to its end, or collect a child
//! already started. The cancellable one, which owns its child's process
//! group, is in `cancellable`.

#[cfg(feature = "async")]
mod cancellable;

use std::fmt::Display;

#[cfg(feature = "async")]
pub use cancellable::output_cancellable;

use crate::Kind;

/// The waits these functions make, spelled as every product spells its
/// mechanism. A child's wait is measured under the program it runs
/// and its subcommand (`crate::measure::program`): `git status` and `git
/// clone` are two operations, every `git status` one.
mod wait {
    #[cfg(feature = "async")]
    use std::future::Future;

    use crate::Kind;

    /// [`crate::blocking`] for a child measured under `operation`.
    pub(super) fn blocking<T, E: std::error::Error>(
        operation: String,
        what: String,
        place: String,
        work: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, E> {
        let waiting = crate::start(Kind::Process, what, place, None, Some(operation));
        waiting.settle(work())
    }

    /// [`crate::until`] for a child measured under `operation`.
    #[cfg(feature = "async")]
    pub(super) async fn until<T, E, F>(
        operation: String,
        what: String,
        place: String,
        work: F,
    ) -> Result<T, E>
    where
        E: std::error::Error,
        F: Future<Output = Result<T, E>>,
    {
        let waiting = crate::start(Kind::Process, what, place, None, Some(operation));
        waiting.settle(work.await)
    }

    /// [`crate::blocking`] and [`crate::until`] for a wait its caller
    /// names, measured under that name.
    pub(super) use crate::blocking as named;
    #[cfg(feature = "async")]
    pub(super) use crate::until as named_async;
}

/// The program and arguments `command` runs, as a person would type them.
pub(crate) fn command_line(command: &std::process::Command) -> String {
    std::iter::once(command.get_program())
        .chain(command.get_args())
        .map(|part| part.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Where `command` runs: its directory, or this process's own when none is
/// set.
pub(crate) fn directory(command: &std::process::Command) -> String {
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
    let what = command_line(command);
    let operation = crate::measure::program(command);
    wait::blocking(operation, what, place, || command.output())
}

/// The same for a `tokio` command: run it to its end and collect what it
/// printed, saying so at both ends.
#[cfg(feature = "async")]
pub async fn output_async(
    command: &mut tokio::process::Command,
) -> std::io::Result<std::process::Output> {
    let standard = command.as_std();
    let place = directory(standard);
    let what = command_line(standard);
    let operation = crate::measure::program(standard);
    wait::until(operation, what, place, command.output()).await
}

/// Run `command` to its end with its output going where it was told to,
/// saying so at both ends.
pub fn status(command: &mut std::process::Command) -> std::io::Result<std::process::ExitStatus> {
    let place = directory(command);
    let what = command_line(command);
    let operation = crate::measure::program(command);
    wait::blocking(operation, what, place, || command.status())
}

/// The same for a `tokio` command.
#[cfg(feature = "async")]
pub async fn status_async(
    command: &mut tokio::process::Command,
) -> std::io::Result<std::process::ExitStatus> {
    let standard = command.as_std();
    let place = directory(standard);
    let what = command_line(standard);
    let operation = crate::measure::program(standard);
    wait::until(operation, what, place, command.status()).await
}

/// Wait for a child already started — `what` names it, as its caller knows
/// it, and is the operation it is measured under — and collect what it
/// printed, saying so at both ends.
pub fn child_output(
    child: std::process::Child,
    what: impl Display,
) -> std::io::Result<std::process::Output> {
    let place = format!("pid {}", child.id());
    wait::named(Kind::Process, what, place, || child.wait_with_output())
}

/// Run `command` to its end with `input` on its stdin and collect what it
/// printed, saying so at both ends. The input is written from its own thread
/// while the outputs are read: written first, an input beyond a pipe buffer
/// left the caller blocked on stdin while a child that prints as it reads
/// was blocked on stdout. A child that closes its stdin before reading all of
/// it has answered without it; that broken pipe is not a failure, and any
/// other write failure is refused naming the command.
pub fn output_with_input(
    command: &mut std::process::Command,
    input: &[u8],
) -> std::io::Result<std::process::Output> {
    use std::io::Write;
    use std::process::Stdio;
    let place = directory(command);
    let what = command_line(command);
    let operation = crate::measure::program(command);
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        std::io::Error::other(format!("{what} was started without a stdin to write to"))
    })?;
    let owned = input.to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&owned));
    let output = wait::blocking(operation, what.clone(), place, || child.wait_with_output())?;
    let written = writer.join().map_err(|payload| {
        let said = match (
            payload.downcast_ref::<String>(),
            payload.downcast_ref::<&'static str>(),
        ) {
            (Some(text), _) => text.clone(),
            (None, Some(text)) => (*text).to_string(),
            (None, None) => "a panic payload that carries no text".to_string(),
        };
        std::io::Error::other(format!("the stdin writer of {what} panicked: {said}"))
    })?;
    match written {
        Ok(()) => Ok(output),
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(output),
        Err(error) => Err(std::io::Error::new(
            error.kind(),
            format!("the input of {what} could not be written: {error}"),
        )),
    }
}

/// The same for a `tokio` child already started: `what` names it, as its
/// caller knows it.
#[cfg(feature = "async")]
pub async fn child_output_async(
    child: tokio::process::Child,
    what: impl Display,
) -> std::io::Result<std::process::Output> {
    let place = match child.id() {
        Some(pid) => format!("pid {pid}"),
        None => "a child that has already ended".to_string(),
    };
    wait::named_async(Kind::Process, what, place, child.wait_with_output()).await
}
