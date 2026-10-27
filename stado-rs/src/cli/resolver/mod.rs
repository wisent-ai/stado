//! `stado resolver` — logical service discovery and local data plane.

use std::sync::Arc;

use clap::Subcommand;
use serde_json::json;

use crate::service_resolution;
use crate::targets::RegistryStore;

use crate::cli::CmdError;

mod accept;
mod api_bind;
mod authority;
mod directory;
mod relay;
mod report;
mod serve;
mod takeover;

pub use crate::cli::resolver::directory::document::canonical_document;
pub use crate::cli::resolver::directory::document::canonical_document_or_last_good;
pub use crate::cli::resolver::serve::serve;
pub(crate) use report::follow::{follow_generation, follow_written};
pub(crate) use report::serving::{await_serving, ensure_upstream_follows, follow_authority};
pub(crate) use takeover::{
    install as install_authority, own_command, promote as promote_authority, take_authority,
    Handover as AuthorityHandover, Installed as InstalledAuthority,
};

pub(crate) use crate::cli::resolver::directory::document::{
    authority_snapshot, directory_document, held_or_authority_snapshot, kept_last_good_document,
    last_good_document,
};
pub(crate) use crate::cli::resolver::directory::remembered::recalled::forget as forget_recalled_registry;
pub(crate) use crate::cli::resolver::directory::remembered::shared::{
    authority_store_elsewhere, store_on_silent_authority,
};
pub(crate) use crate::cli::resolver::directory::source::current_target;
pub(crate) use crate::cli::resolver::directory::write::authority_compare_and_swap;
pub(crate) use crate::cli::resolver::directory::{read_local_document, read_local_snapshot};
pub(crate) use crate::cli::resolver::report::readers::{
    not_serving_reason, published_adapter_url, readiness_marker, unserved_marker,
};
pub(crate) use crate::cli::resolver::report::vault::{KeyringLockRepair, VaultConsultations};
pub(crate) use crate::cli::resolver::report::waiting::host::{
    held_on_host, stood_elsewhere, Stood,
};
pub(crate) use crate::cli::resolver::report::waiting::inflight::{
    bound_seconds, unless_held, Route,
};
pub(crate) use crate::cli::resolver::report::waiting::{
    declared_bound_seconds, held_at, held_to_service,
};

use crate::cli::resolver::directory::emit_snapshot;
use crate::cli::resolver::report::readiness::status;

#[derive(Debug, Subcommand)]
pub enum ResolverCommands {
    /// Resolve one logical service for an authorized workload.
    Resolve {
        /// Logical service name, with or without the stado://service/ prefix.
        service: String,
        /// Stable workload identity from the service unit.
        #[arg(long)]
        consumer: String,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Whether this host's resolver is ready, and why not when it is not.
    ///
    /// Reads the registry and the state the `serve --resolver` role publishes
    /// to [`state_path`],
    /// so diagnostics remain available when the resolver's own API is down.
    /// A non-ready result exits non-zero. Every wait an adapter still stands
    /// on is listed under `waiting_opens` with its service, consumer, bind,
    /// destination host, endpoint and phase: `open` until the host answers
    /// the channel open, `answer` from the client's first byte up the channel
    /// until the service's first byte back. One waiting longer than the
    /// target's declared refresh interval is a blocker, because its client
    /// holds an accepted connection that receives nothing, and every request
    /// that would go through that adapter is refused with its sentence until
    /// it ends. The live process's `/health` endpoint remains the check for
    /// workloads already connected to it.
    Status {
        /// Registry target whose resolver to report on. Defaults to the
        /// target the published state names, then to this host's identity.
        #[arg(long)]
        target: Option<String>,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Read local published channel waits without registry access or network probes.
    Channels {
        /// Exact adapter bind (host:port), when restricting to one route.
        #[arg(long)]
        bind: Option<String>,
        /// Exact consumer identity, when restricting to one workload.
        #[arg(long)]
        consumer: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// One caller's own route to a service placed on another host, carried
    /// over this process's SSH session to that host while the caller holds
    /// this process (its standard input open). Prints the loopback URL to
    /// dial first, as one line (`--json`: one document). For a service placed
    /// on this host the service's own loopback URL is printed and the process
    /// ends. The route the host resolver's adapter would give, when that
    /// resolver holds no listener for it.
    Relay {
        /// Logical service name, with or without the stado://service/ prefix.
        service: String,
        /// Stable workload identity the directory grants the service to.
        #[arg(long)]
        consumer: String,
        /// The loopback host:port to listen on: one this host hands out now
        /// (`stado host local port`).
        #[arg(long)]
        bind: String,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Accept one directory generation the authority answers below the one
    /// this host's resolver holds, after its rollback refusal named both
    /// store versions. The serving resolver adopts exactly that generation on
    /// its next refresh, says so, and forgets the acceptance; any other
    /// generation stays refused.
    Accept {
        /// The generation the authority answers, as the refusal names it.
        #[arg(long)]
        generation: u64,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Ask this host's resolver to read the registry now and wait until it
    /// holds directory GENERATION or a later one; without --generation, the
    /// one the registry authority serves, after the resolver this host reads
    /// from follows its own. Refused with the resolver's own answer.
    Follow {
        /// The directory generation to hold; omitted, the registry the authority serves.
        #[arg(long)]
        generation: Option<u64>,
    },
    /// The host's resolution API.
    Api {
        #[command(subcommand)]
        command: ResolverApiCommands,
    },
    /// What this machine remembers about reading its remote registry
    /// authority: a read given up, or one no SSH path reached, which turns
    /// every next reader to its copy until the refresh interval passes, and
    /// the snapshot kept for that interval.
    Remembered {
        #[command(subcommand)]
        command: ResolverRememberedCommands,
    },
    /// Emit this host's versioned registry for authenticated resolver peers.
    #[command(hide = true)]
    Snapshot,
}

#[derive(Debug, Subcommand)]
pub enum ResolverRememberedCommands {
    /// Show the remembered given-up read and the kept snapshot.
    Show {
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Drop both, so the next command asks the authority at once; prints
    /// what was dropped.
    Forget {
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum ResolverApiCommands {
    /// Give TARGET's resolution API a loopback port that TARGET hands out now
    /// (`stado host local port` run there), recorded as
    /// `targets.<target>.service_resolver.api_bind` under the generation it
    /// was read at. The host's resolver rebinds its API when it reads the
    /// registry; consumers ask the resolver at every start.
    Reassign {
        /// Registry target whose resolver API moves.
        #[arg(long)]
        target: String,
        /// Emit the previous and new bind and the generation as JSON.
        #[arg(long)]
        json: bool,
    },
}

pub async fn dispatch(command: ResolverCommands) -> Result<(), CmdError> {
    match command {
        ResolverCommands::Resolve {
            service,
            consumer,
            json,
        } => resolve_once(&service, &consumer, json).await,
        ResolverCommands::Snapshot => emit_snapshot().await,
        ResolverCommands::Status { target, json } => status(target.as_deref(), json).await,
        ResolverCommands::Channels {
            bind,
            consumer,
            json,
        } => report::waiting::inspect::channels(bind.as_deref(), consumer.as_deref(), json),
        ResolverCommands::Relay {
            service,
            consumer,
            bind,
            json,
        } => relay::relay(&service, &consumer, &bind, json).await,
        ResolverCommands::Accept { generation, json } => accept::accept(generation, json),
        ResolverCommands::Follow { generation } => {
            let generation = match generation {
                Some(generation) => {
                    tokio::task::spawn_blocking(move || follow_generation(generation))
                        .await
                        .map_err(|error| CmdError::click(format!("the wait for this host's resolver did not end: {error}")))?
                        .map_err(CmdError::refused)?;
                    generation
                }
                None => follow_authority(true).await?,
            };
            println!("this host's resolver holds directory generation {generation} or later");
            Ok(())
        }
        ResolverCommands::Api {
            command: ResolverApiCommands::Reassign { target, json },
        } => api_bind::api_reassign(&target, json).await,
        ResolverCommands::Remembered { command } => match command {
            ResolverRememberedCommands::Show { json } => directory::remembered::verbs::show(json),
            ResolverRememberedCommands::Forget { json } => {
                directory::remembered::verbs::forget(json)
            }
        },
    }
}

fn logical_name(value: &str) -> Result<&str, CmdError> {
    // Both spellings name the service: the bare logical name, and the
    // stado://service/<name> address a workload uses.
    let value = match value.strip_prefix("stado://service/") {
        Some(name) => name,
        None => value,
    }
    .trim();
    if value.is_empty() || value.contains('/') {
        return Err(CmdError::usage(
            "SERVICE must be one logical name or stado://service/<name>",
        ));
    }
    Ok(value)
}

/// The directory as this host reads it ([`directory_document`]) and the
/// service resolved for `consumer` from it, with this host's own target name.
async fn resolved_for(
    service: &str,
    consumer: &str,
) -> Result<(service_resolution::ResolvedService, String), CmdError> {
    let service = logical_name(service)?;
    let store = Arc::new(RegistryStore::open().await?);
    let (bootstrap, _, _) = read_local_snapshot(&store).await?;
    let target = current_target(&bootstrap).map_err(CmdError::declaration)?;
    let document = directory_document(store, &bootstrap, &target).await?;
    let resolved =
        service_resolution::resolve(&document, service, consumer).map_err(CmdError::from)?;
    Ok((resolved, target))
}

async fn resolve_once(service: &str, consumer: &str, json_output: bool) -> Result<(), CmdError> {
    let (resolved, _) = resolved_for(service, consumer).await?;
    let uri = format!("stado://service/{}", resolved.name);
    if json_output {
        let report = json!({
            "service": uri,
            "generation": resolved.generation,
            "capabilities": resolved.capabilities,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{uri} generation={} capabilities={}",
            resolved.generation,
            resolved.capabilities.join(",")
        );
    }
    Ok(())
}
