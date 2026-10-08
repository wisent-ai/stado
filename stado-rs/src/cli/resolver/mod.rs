//! `stado resolver` — logical service discovery and local data plane.

use std::sync::Arc;

use clap::Subcommand;
use serde_json::json;

use crate::monitor::host_silence;
use crate::service_resolution;
use crate::targets::RegistryStore;

use crate::cli::CmdError;

mod api_bind;
mod authority;
mod directory;
mod report;
mod serve;

pub use crate::cli::resolver::directory::document::canonical_document;
pub use crate::cli::resolver::directory::document::canonical_document_or_last_good;
pub use crate::cli::resolver::serve::serve;
pub(crate) use report::serving::await_serving;

pub(crate) use crate::cli::resolver::directory::document::last_good_document;
pub(crate) use crate::cli::resolver::directory::source::current_target;
pub(crate) use crate::cli::resolver::directory::source::snapshot_source;
pub(crate) use crate::cli::resolver::directory::{read_local_document, read_local_snapshot};
pub(crate) use crate::cli::resolver::report::published::published_adapter_url;
pub(crate) use crate::cli::resolver::report::published::readiness_marker;

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
    /// A non-ready result exits non-zero. Every channel open an adapter is
    /// still waiting on is listed under `waiting_opens` with its service,
    /// consumer, destination host and endpoint; one waiting longer than the
    /// target's declared refresh interval is a blocker, because its client
    /// holds an accepted connection that receives nothing. The live process's
    /// `/health` endpoint remains the check for workloads already connected to it.
    Status {
        /// Registry target whose resolver to report on. Defaults to the
        /// target the published state names, then to this host's identity.
        #[arg(long)]
        target: Option<String>,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Give TARGET's resolution API a loopback port that TARGET hands out now
    /// (`stado host free-port-local` run there), recorded as
    /// `targets.<target>.service_resolver.api_bind` under the generation it
    /// was read at. The host's resolver rebinds its API when it reads the
    /// registry; consumers ask the resolver at every start.
    #[command(name = "api-reassign")]
    ApiReassign {
        /// Registry target whose resolver API moves.
        #[arg(long)]
        target: String,
        /// Emit the previous and new bind and the generation as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Emit this host's versioned registry for authenticated resolver peers.
    #[command(hide = true)]
    Snapshot,
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
        ResolverCommands::ApiReassign { target, json } => {
            api_bind::api_reassign(&target, json).await
        }
    }
}

fn logical_name(value: &str) -> Result<&str, CmdError> {
    let value = value
        .strip_prefix("stado://service/")
        .unwrap_or(value)
        .trim();
    if value.is_empty() || value.contains('/') {
        return Err(CmdError::usage(
            "SERVICE must be one logical name or stado://service/<name>",
        ));
    }
    Ok(value)
}

async fn resolve_once(service: &str, consumer: &str, json_output: bool) -> Result<(), CmdError> {
    let service = logical_name(service)?;
    let store = Arc::new(RegistryStore::open().await?);
    let (bootstrap, _, _) = read_local_snapshot(&store).await?;
    let target = current_target(&bootstrap).map_err(CmdError::declaration)?;
    let source =
        snapshot_source(Some(store), &bootstrap, &target).map_err(CmdError::declaration)?;
    let (document, _, _) = source.fetch(host_silence::READER_CLI).await?;
    let resolved =
        service_resolution::resolve(&document, service, consumer).map_err(CmdError::from)?;
    let report = json!({
        "service": format!("stado://service/{}", resolved.name),
        "generation": resolved.generation,
        "capabilities": resolved.capabilities,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{} generation={} capabilities={}",
            report["service"].as_str().unwrap_or_default(),
            resolved.generation,
            resolved.capabilities.join(",")
        );
    }
    Ok(())
}
