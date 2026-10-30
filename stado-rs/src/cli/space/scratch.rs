//! `stado space scratch register|list|forget`: the register of trees a job,
//! an agent or a Stado verb made on this machine and declared the janitor may
//! remove once the session that owns each has ended.

use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde_json::json;

use super::coverage::gib;
use super::report::print_json;
use crate::cli::CmdError;
use crate::providers::local::space_reclaim::registered_scratch;

/// Register, list and forget scratch on this machine.
#[derive(Subcommand)]
pub enum SpaceScratchCommands {
    /// Register PATH as scratch that CREATOR made for an owner session; at the disk-full rule the janitor removes it once that session's process has ended.
    Register {
        path: PathBuf,
        /// Who made it: a product, an agent or a verb.
        #[arg(long)]
        creator: String,
        /// Why it exists.
        #[arg(long)]
        purpose: String,
        /// The session that owns it: an agent session, a job id, a run id.
        #[arg(long)]
        owner_session: String,
        /// The owner session's process; the tree is kept while it runs. It
        /// is the agent's or job's own process: a tool call's shell ends
        /// with the command, which would hand the tree over at once.
        #[arg(long)]
        owner_pid: i32,
        #[arg(long)]
        json: bool,
    },
    /// Every registered tree with its size, last change and owner, in the order the janitor takes them.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Forget PATH without touching it.
    Forget {
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

/// The register refused the request; its message names what and how to fix
/// it.
fn refused(error: std::io::Error, json: bool) -> CmdError {
    let message = error.to_string();
    match error.kind() {
        std::io::ErrorKind::InvalidInput => CmdError::usage(message),
        _ => CmdError::declaration(message),
    }
    .machine_readable(json)
}

/// `path` made absolute against the working directory, without resolving
/// links: the register refuses a path that does not spell its real tree.
fn absolute(path: &Path, json: bool) -> Result<PathBuf, CmdError> {
    std::path::absolute(path).map_err(|error| {
        CmdError::usage(format!(
            "{} cannot be made absolute: {error}; name the tree from the root",
            path.display()
        ))
        .machine_readable(json)
    })
}

pub(super) fn scratch(command: SpaceScratchCommands) -> Result<(), CmdError> {
    let json = match &command {
        SpaceScratchCommands::Register { json, .. }
        | SpaceScratchCommands::List { json }
        | SpaceScratchCommands::Forget { json, .. } => *json,
    };
    let home = crate::config_file::home().map_err(|cause| {
        CmdError::declaration(format!(
            "the account's home cannot be named, so its scratch register cannot be found: \
             {cause}; set HOME to this account's home directory"
        ))
        .machine_readable(json)
    })?;
    match command {
        SpaceScratchCommands::Register {
            path,
            creator,
            purpose,
            owner_session,
            owner_pid,
            json,
        } => {
            let entry = registered_scratch::register(
                &home,
                &absolute(&path, json)?,
                &creator,
                &purpose,
                &owner_session,
                owner_pid,
            )
            .map_err(|error| refused(error, json))?;
            if json {
                return print_json(&serde_json::to_value(&entry)?);
            }
            println!(
                "registered {} as scratch {} made for session {} (process {}): {}",
                entry.path.display(),
                entry.creator,
                entry.owner_session,
                entry.owner_pid,
                entry.purpose
            );
            Ok(())
        }
        SpaceScratchCommands::List { json } => {
            let states = registered_scratch::states(&home).map_err(|error| refused(error, json))?;
            if json {
                return print_json(&json!({ "entries": states }));
            }
            for state in states {
                println!(
                    "{:>10}  {}  owner {}  {}  {}  {}: {}",
                    state.bytes.map_or_else(|| "gone".to_string(), gib),
                    state.modified_at.as_deref().unwrap_or("gone"),
                    if state.owner_running {
                        "running"
                    } else {
                        "ended"
                    },
                    state.entry.owner_session,
                    state.entry.path.display(),
                    state.entry.creator,
                    state.entry.purpose
                );
            }
            Ok(())
        }
        SpaceScratchCommands::Forget { path, json } => {
            let path = absolute(&path, json)?;
            let forgotten =
                registered_scratch::forget(&home, &path).map_err(|error| refused(error, json))?;
            if json {
                return print_json(&json!({"path": path, "forgotten": forgotten}));
            }
            if forgotten {
                println!("forgot {}; the tree is untouched", path.display());
            } else {
                println!("{} was not registered", path.display());
            }
            Ok(())
        }
    }
}
