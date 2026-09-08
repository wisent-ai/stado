//! The records a route is compared against: the tunnel's own configuration,
//! its active connector sessions, the one active zone, and the exact DNS
//! records — either for one hostname or for everything the tunnel already owns.

use serde_json::{Map, Value};

use super::api::{exact_zone_id, result_array, TunnelAccess};
use crate::cli::CmdError;

pub(super) struct TunnelConnections {
    pub(super) connector_count: usize,
    pub(super) active_connections: usize,
}

impl TunnelConnections {
    pub(super) fn connected(&self) -> bool {
        self.active_connections > 0
    }
}

pub(super) async fn tunnel_configuration(access: &TunnelAccess) -> Result<Value, CmdError> {
    let current = access.client.get(&access.configuration_path(), &[]).await?;
    Ok(current
        .pointer("/result/config")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new())))
}

pub(super) async fn active_zone_id(access: &TunnelAccess, zone: &str) -> Result<String, CmdError> {
    let zones = access
        .client
        .get(
            "/zones",
            &[("name", zone), ("status", "active"), ("per_page", "50")],
        )
        .await?;
    exact_zone_id(&zones, zone)
}

pub(super) async fn tunnel_connections(
    access: &TunnelAccess,
) -> Result<TunnelConnections, CmdError> {
    let payload = access.client.get(&access.connections_path(), &[]).await?;
    let connectors = result_array(&payload, "Cloudflare tunnel connections")?;
    let active_connections = connectors
        .iter()
        .filter_map(|connector| connector.get("conns").and_then(Value::as_array))
        .map(Vec::len)
        .sum();
    Ok(TunnelConnections {
        connector_count: connectors.len(),
        active_connections,
    })
}

pub(super) async fn exact_dns_records(
    access: &TunnelAccess,
    zone_id: &str,
    hostname: &str,
) -> Result<Vec<Value>, CmdError> {
    let path = format!("/zones/{zone_id}/dns_records");
    let payload = access
        .client
        .get(&path, &[("name", hostname), ("per_page", "100")])
        .await?;
    Ok(result_array(&payload, "Cloudflare DNS record lookup")?.clone())
}

pub(super) fn is_tunnel_dns_record(record: &Value, expected_content: &str) -> bool {
    record.get("type").and_then(Value::as_str) == Some("CNAME")
        && record
            .get("content")
            .and_then(Value::as_str)
            .is_some_and(|content| content.eq_ignore_ascii_case(expected_content))
}

pub(super) async fn tunnel_dns_records(
    access: &TunnelAccess,
    zone_id: &str,
) -> Result<Vec<Value>, CmdError> {
    let path = format!("/zones/{zone_id}/dns_records");
    let content = access.dns_content();
    let payload = access
        .client
        .get(
            &path,
            &[
                ("type", "CNAME"),
                ("content.exact", content.as_str()),
                ("per_page", "5000000"),
            ],
        )
        .await?;
    Ok(result_array(&payload, "Cloudflare tunnel DNS record lookup")?.clone())
}
