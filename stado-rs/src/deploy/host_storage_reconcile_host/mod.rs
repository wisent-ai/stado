//! `stado host local reconcile step …`: the host-side steps of the
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
    /// A unit file of the host's init system: snapshot it, or put a snapshot
    /// back.
    #[command(subcommand)]
    Unit(UnitCommands),
    /// The loopback listener of the object API.
    #[command(subcommand)]
    Listener(ListenerCommands),
    /// The object API's runtime state, as the transaction payload.
    #[command(subcommand)]
    Object(ObjectCommands),
    /// The store the object API serves.
    #[command(subcommand)]
    Store(StoreCommands),
}

/// A unit file of the host's init system.
#[derive(Subcommand)]
pub enum UnitCommands {
    /// Print `STADO_UNIT_SNAPSHOT` with a unit file's bytes and owner.
    Snapshot {
        #[arg(long)]
        path: String,
    },
    /// Put captured unit bytes (base64 in STADO_UNIT_BODY) back in place.
    Restore {
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
}

/// The loopback listener of the object API.
#[derive(Subcommand)]
pub enum ListenerCommands {
    /// Return once nothing listens on the loopback port.
    Closed {
        #[arg(long)]
        port: u16,
    },
}

/// The object API's runtime state.
#[derive(Subcommand)]
pub enum ObjectCommands {
    /// Print the object API's runtime state as the transaction payload.
    Runtime {
        #[arg(long)]
        port: u16,
    },
}

/// The store the object API serves.
#[derive(Subcommand)]
pub enum StoreCommands {
    /// Name the physical root the object API serves; the preflight
    /// inventories arrive base64-encoded on stdin.
    Served {
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
        HostCommands::Unit(UnitCommands::Snapshot { path }) => units::unit_snapshot(&path),
        HostCommands::Unit(UnitCommands::Restore {
            path,
            sha256,
            mode,
            uid,
            gid,
        }) => units::restore_unit(&path, &sha256, mode, uid, gid),
        HostCommands::Listener(ListenerCommands::Closed { port }) => units::listener_closed(port),
        HostCommands::Object(ObjectCommands::Runtime { port }) => {
            served::object_runtime(port).await
        }
        HostCommands::Store(StoreCommands::Served { port }) => served::served_store(port).await,
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
