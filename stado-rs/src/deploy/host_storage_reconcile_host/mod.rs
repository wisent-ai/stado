//! `stado host storage-root-reconcile-host …`: the host-side steps of the
//! storage-root transaction that are not phases of its receipt — launching
//! the resident worker under the native manager, reading and restoring unit
//! files, watching a listener close, and reading what the object API serves.
//! The transaction runs them through the host channel with its own staged
//! tool, so the operator's binary and the host's steps are one revision.
//!
//! Each prints the marker line its caller reads. A refusal is printed on
//! stderr and exits nonzero, which the caller reports as the step's failure.

mod launch;
mod served;
mod units;

use std::process::{Command, Output, Stdio};

use clap::Subcommand;

#[derive(Subcommand)]
pub enum HostCommands {
    /// Start the resident transaction worker under launchd or systemd, or
    /// acknowledge the one already running. The worker arguments arrive
    /// base64-encoded JSON in --arguments.
    Launch {
        #[arg(long)]
        transaction: String,
        #[arg(long)]
        staged: String,
        #[arg(long)]
        tool: String,
        #[arg(long)]
        sha256: String,
        #[arg(long)]
        arguments: String,
    },
    /// Print `STADO_UNIT_SNAPSHOT` with a unit file's bytes and owner.
    UnitSnapshot {
        #[arg(long)]
        path: String,
    },
    /// Put captured unit bytes (base64 in STADO_UNIT_BODY) back in place.
    RestoreUnit {
        #[arg(long)]
        path: String,
        #[arg(long)]
        sha256: String,
        #[arg(long)]
        mode: u32,
        #[arg(long)]
        uid: u32,
        #[arg(long)]
        gid: u32,
    },
    /// Return once nothing listens on the loopback port.
    ListenerClosed {
        #[arg(long)]
        port: u16,
    },
    /// Print the object API's runtime state as the transaction payload.
    ObjectRuntime {
        #[arg(long)]
        port: u16,
    },
    /// Name the physical root the object API serves; the preflight
    /// inventories arrive base64-encoded on stdin.
    ServedStore {
        #[arg(long)]
        port: u16,
    },
}

pub async fn dispatch(command: HostCommands) -> Result<(), String> {
    match command {
        HostCommands::Launch {
            transaction,
            staged,
            tool,
            sha256,
            arguments,
        } => launch::launch(&launch::Request {
            transaction: &transaction,
            staged: &staged,
            tool: &tool,
            sha256: &sha256,
            arguments: &arguments,
        }),
        HostCommands::UnitSnapshot { path } => units::unit_snapshot(&path),
        HostCommands::RestoreUnit {
            path,
            sha256,
            mode,
            uid,
            gid,
        } => units::restore_unit(&path, &sha256, mode, uid, gid),
        HostCommands::ListenerClosed { port } => units::listener_closed(port),
        HostCommands::ObjectRuntime { port } => served::object_runtime(port).await,
        HostCommands::ServedStore { port } => served::served_store(port).await,
    }
}

fn home() -> Result<String, String> {
    std::env::var("HOME").map_err(|_| "HOME is not set".to_string())
}

/// A path as the transaction writes it (`~/…`, `$HOME/…` or `${HOME}/…`)
/// with the home directory put in.
fn expand_home(path: &str) -> Result<String, String> {
    for prefix in ["$HOME", "${HOME}", "~"] {
        if let Some(rest) = path.strip_prefix(prefix) {
            if rest.is_empty() || rest.starts_with('/') {
                return Ok(format!("{}{rest}", home()?));
            }
        }
    }
    Ok(path.to_string())
}

/// Run a program with no stdin; an exit status outside `accepted` refuses
/// with the last line it wrote.
fn checked(argv: &[&str], accepted: &[i32]) -> Result<Output, String> {
    let (program, arguments) = argv
        .split_first()
        .ok_or_else(|| "no program to run".to_string())?;
    let output = crate::wait::output(Command::new(program).args(arguments).stdin(Stdio::null()))
        .map_err(|error| format!("cannot run {program}: {error}"))?;
    if output
        .status
        .code()
        .is_some_and(|code| accepted.contains(&code))
    {
        return Ok(output);
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    Err(detail
        .trim()
        .lines()
        .last()
        .unwrap_or("native service command failed")
        .to_string())
}
