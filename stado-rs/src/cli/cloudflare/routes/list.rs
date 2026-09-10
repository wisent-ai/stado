//! The zone-wide route inventory: every hostname the tunnel configures plus
//! every hostname its DNS already points at, inspected one by one.

use serde_json::{json, Value};

use super::ingress::configured_hostnames;
use super::inspect::inspect_route;
use crate::cli::cloudflare::api::{belongs_to_zone, tunnel_access, validate_dns_name};
use crate::cli::cloudflare::records::{
    active_zone_id, tunnel_configuration, tunnel_connections, tunnel_dns_records,
};
use crate::cli::{reporting::table, CmdError};

pub(in crate::cli::cloudflare) async fn list_routes(
    api_credential_name: &str,
    tunnel_credential_name: &str,
    zone: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    validate_dns_name("zone", zone)?;
    let access = tunnel_access(api_credential_name, tunnel_credential_name).await?;
    let config = tunnel_configuration(&access).await?;
    let zone_id = active_zone_id(&access, zone).await?;
    let connections = tunnel_connections(&access).await?;
    let mut hostnames = configured_hostnames(&config, zone)?;
    hostnames.extend(
        tunnel_dns_records(&access, &zone_id)
            .await?
            .iter()
            .filter_map(|record| record.get("name").and_then(Value::as_str))
            .filter(|hostname| belongs_to_zone(hostname, zone))
            .map(str::to_string),
    );
    hostnames.sort_unstable();
    hostnames.dedup();

    let mut routes = Vec::with_capacity(hostnames.len());
    for hostname in hostnames {
        routes.push(
            inspect_route(
                &access,
                &config,
                &zone_id,
                &hostname,
                connections.connected(),
            )
            .await?,
        );
    }
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "status": "listed",
                "api_credential": api_credential_name,
                "tunnel_credential": tunnel_credential_name,
                "account_id": access.account_id,
                "tunnel_id": access.tunnel_id,
                "zone": zone,
                "zone_id": zone_id,
                "connector_count": connections.connector_count,
                "active_connections": connections.active_connections,
                "tunnel_connected": connections.connected(),
                "routes": routes,
            }))?
        );
    } else {
        let rows: Vec<Vec<String>> = routes
            .iter()
            .map(|route| {
                vec![
                    route.hostname.clone(),
                    route.state.to_string(),
                    route.origin.clone().unwrap_or_else(|| "-".to_string()),
                    route.ingress_rules.to_string(),
                    route.dns_records.to_string(),
                ]
            })
            .collect();
        table::print(&["HOSTNAME", "STATE", "ORIGIN", "INGRESS", "DNS"], &rows);
        println!(
            "{} connector(s), {} active tunnel connection(s)",
            connections.connector_count, connections.active_connections
        );
    }
    Ok(())
}
