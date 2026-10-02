//! The `stado tunnel` command surface and its dispatch table. The command is
//! named for what it does — route public hostnames through a tunnel's
//! ingress and DNS — and the provider that carries the tunnel is an argument,
//! so a second provider is a new `TunnelProvider` value, not a new verb.

use clap::{Args, Subcommand, ValueEnum};

use super::routes::{list_routes, remove_route, route_status, route_tunnel};
use crate::cli::CmdError;

/// The providers a tunnel route can be carried by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum TunnelProvider {
    /// Cloudflare Tunnel: ingress through the account's named tunnel, DNS in
    /// the account's zone, both through the Cloudflare API.
    Cloudflare,
}

#[derive(Args)]
pub struct TunnelScopeArgs {
    /// Provider that carries the tunnel. No provider is assumed.
    #[arg(long, value_enum)]
    provider: TunnelProvider,
    /// Stado credential containing the provider account_id and scoped api_token.
    #[arg(long)]
    api_credential: String,
    /// Stado credential containing the same account_id and tunnel_id.
    #[arg(long)]
    tunnel_credential: String,
    /// Exact DNS zone name the provider serves, for example example.com.
    #[arg(long)]
    zone: String,
}

#[derive(Subcommand)]
pub enum TunnelCommands {
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
    /// Route one hostname to an origin behind the provider's named tunnel.
    Route {
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

pub async fn dispatch(command: TunnelCommands) -> Result<(), CmdError> {
    match command {
        TunnelCommands::List { scope, json } => {
            let TunnelProvider::Cloudflare = scope.provider;
            list_routes(
                &scope.api_credential,
                &scope.tunnel_credential,
                &scope.zone,
                json,
            )
            .await
        }
        TunnelCommands::Status {
            scope,
            hostname,
            json,
        } => {
            let TunnelProvider::Cloudflare = scope.provider;
            route_status(
                &scope.api_credential,
                &scope.tunnel_credential,
                &scope.zone,
                &hostname,
                json,
            )
            .await
        }
        TunnelCommands::Route {
            scope,
            hostname,
            origin,
            host,
            connector_service,
            connector_token_field,
            connector_secret_name,
            json,
        } => {
            let TunnelProvider::Cloudflare = scope.provider;
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
        TunnelCommands::Remove {
            scope,
            hostname,
            json,
        } => {
            let TunnelProvider::Cloudflare = scope.provider;
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
