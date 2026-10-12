//! `stado host local api …`: the host half of the object-API
//! recovery (`deploy/recover_object_api/*.sh`), run by the host's own Stado
//! under the storage-root lock. The shell keeps the launchd, sudo and curl
//! steps; every reader of JSON, plist and process state it needs is here.
//! A refusal is printed on stderr exactly as written, with exit 1, because
//! the recovery reports its last stderr line as the reason.

mod config;
mod definition;
mod route;
mod skarbiec;

use std::path::PathBuf;

use clap::Subcommand;

use crate::cli::{CmdError, CLICK_ERROR_CODE};

#[derive(Subcommand)]
pub enum ObjectApiLocalCommands {
    /// `STORE\tBACKUP_STORE\tOBJECT_URL\tNAMESPACE\tTOKEN_FILE\tLABEL\tRETIRED`
    /// for this host; LABEL is the launchd label of the host Stado process and
    /// RETIRED the comma-separated labels on this host that run the Stado
    /// program as an API listener under another label.
    Paths {
        #[arg(long)]
        config: PathBuf,
    },
    /// The object API's launchd definition: render it, or compare two.
    #[command(subcommand)]
    Plist(PlistCommands),
    /// The object boundary the server reports.
    #[command(subcommand)]
    Boundary(BoundaryCommands),
    /// The storage route of a loaded job (`launchctl print` output) or of a
    /// plist, as twelve tab-separated fields.
    Route {
        #[arg(long)]
        mode: String,
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        config: String,
        #[arg(long)]
        expected: PathBuf,
        #[arg(long)]
        runtime: PathBuf,
    },
    /// The Skarbiec release this host runs beside its object API.
    #[command(subcommand)]
    Skarbiec(SkarbiecCommands),
}

/// The object API's launchd definition.
#[derive(Subcommand)]
pub enum PlistCommands {
    /// Write the object API's launchd definition to --staged, keeping every
    /// option of the installed one that recovery does not own.
    Render {
        #[arg(long)]
        staged: PathBuf,
        #[arg(long)]
        installed: PathBuf,
        #[arg(long)]
        label: String,
        #[arg(long)]
        program: String,
        #[arg(long)]
        store: String,
        #[arg(long)]
        backup_store: String,
        #[arg(long)]
        account: String,
        #[arg(long)]
        log: String,
        #[arg(long)]
        config: String,
    },
    /// Succeed when both definitions parse and are equal.
    Equal { left: PathBuf, right: PathBuf },
}

/// The object boundary the server reports.
#[derive(Subcommand)]
pub enum BoundaryCommands {
    /// Succeed when the server's state reports a ready object boundary with
    /// no error.
    Ready { state: PathBuf },
}

/// The Skarbiec release this host runs beside its object API.
#[derive(Subcommand)]
pub enum SkarbiecCommands {
    /// The registry's Skarbiec release plan for this host, or `absent`.
    Plan {
        #[arg(long)]
        registry: PathBuf,
        #[arg(long)]
        host: String,
        #[arg(long)]
        account: String,
    },
    /// `owned` or `unowned` from the Skarbiec release state.
    Ownership {
        #[arg(long)]
        state: PathBuf,
        #[arg(long)]
        target: String,
    },
    /// The Skarbiec proxy's upstream, when it is a declared candidate.
    Upstream {
        #[arg(long)]
        state: PathBuf,
        #[arg(long)]
        ports: String,
    },
    /// `none` or `exact\tPID\tEXECUTABLE` for the exact orphaned proxy in a
    /// `ps axww -o pid= -o command=` listing.
    Proxy {
        #[arg(long)]
        processes: PathBuf,
        #[arg(long)]
        state: String,
        #[arg(long)]
        bind: String,
    },
}

fn refused(detail: String) -> CmdError {
    eprintln!("{detail}");
    CmdError::silent(CLICK_ERROR_CODE)
}

fn answer(result: Result<String, String>) -> Result<(), CmdError> {
    println!("{}", result.map_err(refused)?);
    Ok(())
}

fn verdict(holds: bool) -> Result<(), CmdError> {
    if holds {
        Ok(())
    } else {
        Err(CmdError::silent(CLICK_ERROR_CODE))
    }
}

pub async fn dispatch(command: ObjectApiLocalCommands) -> Result<(), CmdError> {
    use ObjectApiLocalCommands as C;
    match command {
        C::Paths { config: path } => answer(config::paths(&path).await),
        C::Plist(PlistCommands::Render {
            staged,
            installed,
            label,
            program,
            store,
            backup_store,
            account,
            log,
            config: path,
        }) => {
            let wanted = definition::Definition {
                label: &label,
                program: &program,
                store: &store,
                backup_store: &backup_store,
                account: &account,
                log: &log,
                config: &path,
            };
            definition::render(&installed, &staged, &wanted).map_err(refused)
        }
        C::Plist(PlistCommands::Equal { left, right }) => verdict(definition::same(&left, &right)),
        C::Boundary(BoundaryCommands::Ready { state }) => verdict(config::boundary_ready(&state)),
        C::Route {
            mode,
            source,
            config: path,
            expected,
            runtime,
        } => answer(route::inspect(&route::Inspection {
            mode: &mode,
            source: &source,
            default_config: &path,
            expected: &expected,
            runtime: &runtime,
        })),
        C::Skarbiec(SkarbiecCommands::Plan {
            registry,
            host,
            account,
        }) => answer(skarbiec::plan(&registry, &host, &account)),
        C::Skarbiec(SkarbiecCommands::Ownership { state, target }) => {
            answer(skarbiec::ownership(&state, &target))
        }
        C::Skarbiec(SkarbiecCommands::Upstream { state, ports }) => {
            answer(skarbiec::upstream(&state, &ports))
        }
        C::Skarbiec(SkarbiecCommands::Proxy {
            processes,
            state,
            bind,
        }) => answer(skarbiec::proxy_match(&processes, &state, &bind)),
    }
}
