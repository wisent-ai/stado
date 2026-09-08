//! `stado resolver` — logical service discovery and local data plane.

use std::sync::Arc;

use clap::Subcommand;
use serde_json::json;

use crate::monitor::host_silence;
use crate::service_resolution;
use crate::targets::RegistryStore;

use crate::cli::CmdError;

mod authority;
mod directory;
mod report;
mod serve;

pub use crate::cli::resolver::authority::tunnel::TUNNEL_OPEN_BUDGET;
pub use crate::cli::resolver::directory::document::canonical_document;
pub use crate::cli::resolver::directory::document::canonical_document_or_last_good;
pub use crate::cli::resolver::serve::serve;

pub(crate) use crate::cli::resolver::directory::read_local_snapshot;
pub(crate) use crate::cli::resolver::directory::source::current_target;
pub(crate) use crate::cli::resolver::directory::source::snapshot_source;

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
    /// Run the local resolution API and configured stable-port adapters.
    Serve {
        /// Exact registry target whose service_resolver policy to enforce.
        #[arg(long)]
        target: String,
    },
    /// Whether this host's resolver is ready, and why not when it is not.
    ///
    /// A subcommand rather than another endpoint on the resolver's own API,
    /// for one reason: the question is asked when the resolver is DOWN. On
    /// 2026-08-19 this host's resolver sat in a launchd restart loop holding a
    /// dead ssh control socket, and an answer served on `api_bind` would have
    /// been unreachable for exactly the window an operator needed it. This
    /// reads the registry and the state `serve` publishes to
    /// [`state_path`], so it answers with the resolver stopped, and exits
    /// non-zero when the answer is not `ready` so a unit or a script can act
    /// on it. The live process's own `/health` remains where a workload
    /// checks a resolver it is already talking to.
    Status {
        /// Registry target whose resolver to report on. Defaults to the
        /// target the published state names, then to this host's identity.
        #[arg(long)]
        target: Option<String>,
        /// Emit machine-readable output.
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
        ResolverCommands::Serve { target } => serve(&target).await,
        ResolverCommands::Snapshot => emit_snapshot().await,
        ResolverCommands::Status { target, json } => status(target.as_deref(), json).await,
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
    let (bootstrap, _, _) = read_local_snapshot(&store).await.map_err(CmdError::click)?;
    let target = current_target(&bootstrap).map_err(CmdError::click)?;
    let source = snapshot_source(Some(store), &bootstrap, &target).map_err(CmdError::click)?;
    let (document, _, _) = source
        .fetch(host_silence::READER_CLI)
        .await
        .map_err(CmdError::click)?;
    let resolved =
        service_resolution::resolve(&document, service, consumer).map_err(CmdError::click)?;
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
