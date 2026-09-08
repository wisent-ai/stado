//! The `stado installation cloudflare` command surface and its dispatch table.

use clap::{Args, Subcommand};

use super::routes::{list_routes, remove_route, route_status, route_tunnel};
use crate::cli::CmdError;

#[derive(Args)]
pub struct TunnelScopeArgs {
    /// Stado credential containing the Cloudflare account_id and scoped api_token.
    #[arg(long)]
    api_credential: String,
    /// Stado credential containing the same account_id and tunnel_id.
    #[arg(long)]
    tunnel_credential: String,
    /// Exact Cloudflare zone name, for example bobloo.com.
    #[arg(long)]
    zone: String,
}

#[derive(Subcommand)]
pub enum CloudflareCommands {
    /// List tunnel ingress and DNS state for every hostname in one zone.
    List {
        #[command(flatten)]
        scope: TunnelScopeArgs,
        /// Emit the machine-readable route inventory.
        #[arg(long)]
        json: bool,
    },
    /// Inspect one hostname's ingress, DNS and tunnel connection state.
    Status {
        #[command(flatten)]
        scope: TunnelScopeArgs,
        /// Exact public hostname to inspect.
        #[arg(long)]
        hostname: String,
        /// Emit the machine-readable route report.
        #[arg(long)]
        json: bool,
    },
    /// Route one hostname to an origin behind a named Cloudflare Tunnel.
    #[command(name = "route-tunnel")]
    RouteTunnel {
        #[command(flatten)]
        scope: TunnelScopeArgs,
        /// Exact public hostname to route.
        #[arg(long)]
        hostname: String,
        /// Connector-local HTTP(S) origin, for example http://localhost on port 3000.
        #[arg(long)]
        origin: String,
        /// Registry host running the connector.
        #[arg(long)]
        host: String,
        /// Registry-managed connector service on that host.
        #[arg(long, default_value = "cloudflared")]
        connector_service: String,
        /// Credential field containing the connector token.
        #[arg(long, default_value = "token")]
        connector_token_field: String,
        /// Owner-only token filename under the connector service user's ~/.stado.
        #[arg(long, default_value = "cloudflared-token")]
        connector_secret_name: String,
        /// Emit the nonsecret change report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Remove one hostname's tunnel ingress and matching tunnel DNS records.
    Remove {
        #[command(flatten)]
        scope: TunnelScopeArgs,
        /// Exact public hostname to remove.
        #[arg(long)]
        hostname: String,
        /// Emit the nonsecret removal report as JSON.
        #[arg(long)]
        json: bool,
    },
}

pub async fn dispatch(command: CloudflareCommands) -> Result<(), CmdError> {
    match command {
        CloudflareCommands::List { scope, json } => {
            list_routes(
                &scope.api_credential,
                &scope.tunnel_credential,
                &scope.zone,
                json,
            )
            .await
        }
        CloudflareCommands::Status {
            scope,
            hostname,
            json,
        } => {
            route_status(
                &scope.api_credential,
                &scope.tunnel_credential,
                &scope.zone,
                &hostname,
                json,
            )
            .await
        }
        CloudflareCommands::RouteTunnel {
            scope,
            hostname,
            origin,
            host,
            connector_service,
            connector_token_field,
            connector_secret_name,
            json,
        } => {
            route_tunnel(
                &scope.api_credential,
                &scope.tunnel_credential,
                &scope.zone,
                &hostname,
                &origin,
                &host,
                &connector_service,
                &connector_token_field,
                &connector_secret_name,
                json,
            )
            .await
        }
        CloudflareCommands::Remove {
            scope,
            hostname,
            json,
        } => {
            remove_route(
                &scope.api_credential,
                &scope.tunnel_credential,
                &scope.zone,
                &hostname,
                json,
            )
            .await
        }
    }
}
