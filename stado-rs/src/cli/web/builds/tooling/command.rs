//! One command run in the checkout, and how a failed exit is reported.

use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};

use crate::cli::web::builds::PLATFORM;
use crate::cli::CmdError;

/// How a failed command's exit is reported. A killed process has no code, and
/// "exit status 0" would be a lie about a build that died on SIGKILL because
/// the builder ran out of memory — which is how a Next.js build fails.
pub(super) fn exit_report(status: ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit status {code}"),
        None => status.to_string(),
    }
}

/// Run one command in the checkout with its stdout and stderr inherited, so
/// the release log carries the tool's own diagnostics rather than a Stado
/// paraphrase of them. stdin is closed: a builder has no operator to answer a
/// prompt, and a release that hangs on one holds the worker forever.
///
/// `path` replaces the inherited `PATH`, and exists for one reason: a program
/// that is itself a script needs its interpreter findable, and the only caller
/// that knows where that interpreter is, is the one that just resolved the
/// script.
///
/// `variables` are the platform's declared build-time `env`. The names are
/// printed and the values are not: a value here is public by construction,
/// but a build log is read by everyone and a habit of printing what is in a
/// variable is the habit that eventually prints a credential.
pub(super) fn run_with_path(
    source: &Path,
    program: &str,
    arguments: &[&str],
    toolchain: &str,
    path: Option<&str>,
    variables: &[(String, String)],
) -> Result<(), CmdError> {
    let rendered = format!("{program} {}", arguments.join(" "));
    if variables.is_empty() {
        println!("stado web: {rendered}");
    } else {
        let names: Vec<&str> = variables.iter().map(|(name, _)| name.as_str()).collect();
        println!("stado web: {rendered} (with {})", names.join(", "));
    }
    let mut command = Command::new(program);
    command
        .args(arguments)
        .current_dir(source)
        .stdin(Stdio::null());
    if let Some(path) = path {
        command.env("PATH", path);
    }
    for (name, value) in variables {
        command.env(name, value);
    }
    let status = crate::wait::status(&mut command).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            CmdError::click(format!(
                "`{program}` is not there: the builder running the `{PLATFORM}` platform has no \
                 {toolchain}, so give that platform a `runner_platform` whose host carries one"
            ))
            .stating(crate::primitives::failure::FailureCode::Config)
        } else {
            CmdError::click(format!("cannot run `{rendered}`: {error}"))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        }
    })?;
    if !status.success() {
        return Err(CmdError::refused(format!(
            "`{rendered}` failed with {}",
            exit_report(status)
        )));
    }
    Ok(())
}
