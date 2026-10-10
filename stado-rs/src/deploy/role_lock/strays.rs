//! The Stado role processes running on this host that nothing accounts for:
//! a second copy of a role whose lock another process holds, a process whose
//! program is no longer on disk or is not the installed Stado, and a role
//! command this release no longer has. The host-health pass ends them with
//! SIGKILL and waits for each to be gone, and reports what it ended and why,
//! so a stray never outlives one pass and nobody has to kill it by hand.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::{holders, installed_program};
use crate::cli::CmdError;

/// Why a process is a stray.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StrayReason {
    /// Its program is not the installed Stado: a checkout's build, a copy.
    NotInstalledProgram,
    /// Its program no longer exists on disk.
    ProgramGone,
    /// It runs a role command this release does not have.
    RetiredRole,
    /// A role this process plays is held by another process.
    DuplicateRole,
}

#[derive(Debug, Clone, Serialize)]
pub struct Stray {
    pub pid: u32,
    pub program: PathBuf,
    pub command: String,
    /// How long it has run, as `ps` prints its elapsed time.
    pub running_for: String,
    pub reason: StrayReason,
    pub detail: String,
}

/// The one role command this release's `stado` has; `agent` is the role
/// earlier releases had, retired into `serve --worker`.
const ROLE_COMMAND: &str = "serve";
const RETIRED_ROLE_COMMAND: &str = "agent";

/// One process on this host as `ps` reports it.
struct Process {
    pid: u32,
    running_for: String,
    program: PathBuf,
    /// The word after the program: the role it plays, when it plays one.
    role_word: Option<String>,
    command: String,
}

fn processes() -> Result<Vec<Process>, CmdError> {
    let mut command = std::process::Command::new("ps");
    command.args(["-axo", "pid=,etime=,command="]);
    let output = crate::wait::output(&mut command)
        .map_err(|error| CmdError::unreachable(format!("ps could not be run: {error}")))?;
    if !output.status.success() {
        return Err(CmdError::unreachable(format!(
            "ps exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text.lines().filter_map(parse_line).collect())
}

/// `pid etime program [role-word …]`.
fn parse_line(line: &str) -> Option<Process> {
    let mut words = line.split_whitespace();
    let pid = words.next()?.parse().ok()?;
    let running_for = words.next()?.to_string();
    let program = PathBuf::from(words.next()?);
    let rest: Vec<&str> = words.collect();
    let role_word = rest.first().map(|word| word.to_string());
    let command = std::iter::once(program.display().to_string())
        .chain(rest.iter().map(|word| word.to_string()))
        .collect::<Vec<_>>()
        .join(" ");
    Some(Process {
        pid,
        running_for,
        program,
        role_word,
        command,
    })
}

/// Whether `program` is a Stado program by name.
fn is_stado(program: &Path) -> bool {
    program.file_name().is_some_and(|name| name == "stado")
}

/// Every Stado role process on this host that nothing accounts for, this
/// process left out.
pub fn strays(home: &Path) -> Result<Vec<Stray>, CmdError> {
    let own = std::process::id();
    let installed = installed_program(home);
    let held = holders(home)?;
    let mut found = Vec::new();
    for process in processes()? {
        if process.pid == own || !is_stado(&process.program) {
            continue;
        }
        let Some(role_word) = process.role_word.as_deref() else {
            continue;
        };
        let retired = role_word == RETIRED_ROLE_COMMAND;
        if role_word != ROLE_COMMAND && !retired {
            // Not a role: a one-shot command somebody is running.
            continue;
        }
        let reason = if !process.program.is_file() {
            Some((
                StrayReason::ProgramGone,
                format!("{} no longer exists", process.program.display()),
            ))
        } else if retired {
            Some((
                StrayReason::RetiredRole,
                format!(
                    "`stado {RETIRED_ROLE_COMMAND}` is not a role this release has; the worker \
                     is `stado serve --worker`"
                ),
            ))
        } else if let Some(installed) = installed
            .as_ref()
            .filter(|installed| **installed != process.program)
        {
            Some((
                StrayReason::NotInstalledProgram,
                format!(
                    "{} is not the installed {}",
                    process.program.display(),
                    installed.display()
                ),
            ))
        } else if held.iter().any(|holder| holder.pid == process.pid) {
            None
        } else {
            Some((
                StrayReason::DuplicateRole,
                "holds no role lock; another process holds every role this host runs".to_string(),
            ))
        };
        if let Some((reason, detail)) = reason {
            found.push(Stray {
                pid: process.pid,
                program: process.program,
                command: process.command,
                running_for: process.running_for,
                reason,
                detail,
            });
        }
    }
    Ok(found)
}

/// End every stray with SIGKILL and wait for it to be gone; what was ended
/// is answered, and a stray that could not be ended is an error naming it.
pub fn end_strays(strays: &[Stray]) -> Result<Vec<Stray>, CmdError> {
    let mut ended = Vec::new();
    for stray in strays {
        let pid = nix::unistd::Pid::from_raw(i32::try_from(stray.pid).map_err(|error| {
            CmdError::unreachable(format!("pid {} is not a process id: {error}", stray.pid))
        })?);
        match nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGKILL) {
            Ok(()) | Err(nix::errno::Errno::ESRCH) => {}
            Err(error) => {
                return Err(CmdError::unreachable(format!(
                    "stray pid {} ({}) could not be ended: {error}",
                    stray.pid, stray.command
                )))
            }
        }
        crate::wait::blocking(
            crate::wait::Kind::Process,
            format!("the ended stray pid {} to be gone", stray.pid),
            "this host",
            || gone(pid),
        )?;
        ended.push(stray.clone());
    }
    Ok(ended)
}

/// Block until `pid` no longer exists: a killed process is reaped by its
/// own parent, so this asks the kernel whether it is still there, yielding
/// between asks, and never guesses a duration.
fn gone(pid: nix::unistd::Pid) -> Result<(), CmdError> {
    while nix::sys::signal::kill(pid, None).is_ok() {
        std::thread::yield_now();
    }
    Ok(())
}
