//! The `stado route` command surface and its dispatch table.

use clap::Subcommand;

use super::{forward, inspect};
use crate::cli::CmdError;

#[derive(Debug, Subcommand)]
pub enum RouteCommands {
    /// List every declared service, endpoint and locally open forward.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Open one service's declared forward marker locally or on an endpoint holder.
    Open {
        service: String,
        /// Select which host's declared endpoint to materialize.
        #[arg(long)]
        target: Option<String>,
        /// Write the marker under this process's HOME.
        #[arg(long, conflicts_with = "remote")]
        local: bool,
        /// Write the marker on the selected endpoint holder through its fleet channel.
        #[arg(long, conflicts_with = "local")]
        remote: bool,
        #[arg(long)]
        json: bool,
    },
    /// Close the named service forward and remove its marker.
    Close {
        service: String,
        /// Remove the marker for this declared endpoint holder.
        #[arg(long)]
        target: Option<String>,
    },
    /// Read the capability routes on the host serving SERVICE.
    Capability {
        service: String,
        #[arg(long)]
        json: bool,
    },
    /// Authorize TARGET's resolver key on the declared directory authority.
    Key {
        target: String,
        #[arg(long)]
        json: bool,
    },
    /// Publish host placement policies selected by directory placement.
    #[command(subcommand)]
    Placement(RoutePlacementCommands),
}

#[derive(Debug, Subcommand)]
pub enum RoutePlacementCommands {
    /// Publish to every active host, optionally only mobile-capable hosts.
    Publish {
        #[arg(long)]
        mobile: bool,
        #[arg(long)]
        json: bool,
    },
}

pub async fn dispatch(command: RouteCommands) -> Result<(), CmdError> {
    match command {
        RouteCommands::List { json } => forward::list(json).await,
        RouteCommands::Open {
            service,
            target,
            local,
            remote,
            json,
        } => forward::open(&service, target.as_deref(), local, remote, json).await,
        RouteCommands::Close { service, target } => {
            forward::close(&service, target.as_deref()).await
        }
        RouteCommands::Capability { service, json } => inspect::capability(&service, json).await,
        RouteCommands::Key { target, json } => inspect::key(&target, json).await,
        RouteCommands::Placement(RoutePlacementCommands::Publish { mobile, json }) => {
            inspect::publish_placement(mobile, json).await
        }
    }
}
