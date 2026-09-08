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
    let status = command.status().map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            CmdError::click(format!(
                "`{program}` is not there: the builder running the `{PLATFORM}` platform has no \
                 {toolchain}, so give that platform a `runner_platform` whose host carries one"
            ))
        } else {
            CmdError::click(format!("cannot run `{rendered}`: {error}"))
        }
    })?;
    if !status.success() {
        return Err(CmdError::click(format!(
            "`{rendered}` failed with {}",
            exit_report(status)
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn a_killed_command_is_not_reported_as_a_clean_exit() {
        use std::os::unix::process::ExitStatusExt;

        // A wait status of 7 << 8 is an exit code of 7.
        assert_eq!(exit_report(ExitStatus::from_raw(7 << 8)), "exit status 7");
        // A Next.js build that exhausts the builder's memory is killed and has
        // no exit code at all. Reporting a code there would say the build
        // returned something, and the operator would look for a compile error
        // that was never printed.
        let killed = exit_report(ExitStatus::from_raw(9));
        assert!(killed.contains("signal"), "{killed}");
        assert!(!killed.contains("exit status"), "{killed}");
    }
}
