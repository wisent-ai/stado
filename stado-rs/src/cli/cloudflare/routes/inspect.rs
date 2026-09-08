//! One hostname's route state — its ingress rules, its exact DNS records and
//! the tunnel's connector sessions — and the status command that reports it.

use serde::Serialize;
use serde_json::{json, Value};

use super::ingress::ingress_rules;
use crate::cli::cloudflare::api::{tunnel_access, validate_zone_hostname, TunnelAccess};
use crate::cli::cloudflare::records::{
    active_zone_id, exact_dns_records, is_tunnel_dns_record, tunnel_configuration,
    tunnel_connections,
};
use crate::cli::CmdError;

#[derive(Serialize)]
pub(super) struct RouteInspection {
    pub(super) hostname: String,
    pub(super) origin: Option<String>,
    pub(super) ingress_rules: usize,
    pub(super) dns_records: usize,
    conflicting_dns_records: usize,
    dns_record_ids: Vec<String>,
    dns_content: String,
    proxied: bool,
    tunnel_connected: bool,
    consistent: bool,
    pub(super) state: &'static str,
    origin_reachability: &'static str,
}

pub(super) async fn inspect_route(
    access: &TunnelAccess,
    config: &Value,
    zone_id: &str,
    hostname: &str,
    tunnel_connected: bool,
) -> Result<RouteInspection, CmdError> {
    let matching_ingress: Vec<&Value> = ingress_rules(config)?
        .iter()
        .filter(|rule| rule.get("hostname").and_then(Value::as_str) == Some(hostname))
        .collect();
    let origin = if matching_ingress.len() == 1 {
        matching_ingress[0]
            .get("service")
            .and_then(Value::as_str)
            .map(str::to_string)
    } else {
        None
    };

    let records = exact_dns_records(access, zone_id, hostname).await?;
    let expected_content = access.dns_content();
    let matching_dns: Vec<&Value> = records
        .iter()
        .filter(|record| is_tunnel_dns_record(record, &expected_content))
        .collect();
    let conflicting_dns_records = records
        .iter()
        .filter(|record| {
            let record_type = record.get("type").and_then(Value::as_str);
            matches!(record_type, Some("A" | "AAAA" | "CNAME"))
                && !is_tunnel_dns_record(record, &expected_content)
        })
        .count();
    let dns_record_ids = matching_dns
        .iter()
        .filter_map(|record| record.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    let proxied = matching_dns.len() == 1
        && matching_dns[0].get("proxied").and_then(Value::as_bool) == Some(true);
    let consistent = matching_ingress.len() == 1
        && origin.is_some()
        && matching_dns.len() == 1
        && proxied
        && conflicting_dns_records == 0;
    let state =
        if matching_ingress.is_empty() && matching_dns.is_empty() && conflicting_dns_records == 0 {
            "absent"
        } else if !consistent {
            "drifted"
        } else if !tunnel_connected {
            "connector_down"
        } else {
            "routed"
        };
    Ok(RouteInspection {
        hostname: hostname.to_string(),
        origin,
        ingress_rules: matching_ingress.len(),
        dns_records: matching_dns.len(),
        conflicting_dns_records,
        dns_record_ids,
        dns_content: expected_content,
        proxied,
        tunnel_connected,
        consistent,
        state,
        origin_reachability: "not_probed",
    })
}

pub(in crate::cli::cloudflare) async fn route_status(
    api_credential_name: &str,
    tunnel_credential_name: &str,
    zone: &str,
    hostname: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    validate_zone_hostname(zone, hostname)?;
    let access = tunnel_access(api_credential_name, tunnel_credential_name).await?;
    let config = tunnel_configuration(&access).await?;
    let zone_id = active_zone_id(&access, zone).await?;
    let connections = tunnel_connections(&access).await?;
    let route = inspect_route(
        &access,
        &config,
        &zone_id,
        hostname,
        connections.connected(),
    )
    .await?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "status": "inspected",
                "api_credential": api_credential_name,
                "tunnel_credential": tunnel_credential_name,
                "account_id": access.account_id,
                "tunnel_id": access.tunnel_id,
                "zone": zone,
                "zone_id": zone_id,
                "connector_count": connections.connector_count,
                "active_connections": connections.active_connections,
                "tunnel_connected": connections.connected(),
                "route": route,
            }))?
        );
    } else {
        println!(
            "{}: {} ({} ingress rule(s), {} tunnel DNS record(s), {} active connection(s)); origin reachability not probed",
            route.hostname,
            route.state,
            route.ingress_rules,
            route.dns_records,
            connections.active_connections
        );
    }
    Ok(())
}
