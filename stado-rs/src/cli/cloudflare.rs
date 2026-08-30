//! Native Cloudflare Tunnel route management through credentials held by Stado.
//!
//! Inventory and status compare tunnel ingress, exact DNS records and active
//! connector sessions without claiming that the connector can reach its origin.
//! Upsert configures ingress before moving DNS. Removal deletes only matching
//! tunnel DNS before ingress and preserves the shared connector. Secret values
//! stay in memory and are never rendered in command output.

use clap::{Args, Subcommand};
use reqwest::{Method, RequestBuilder};
use serde::Serialize;
use serde_json::{json, Map, Value};

use super::{table, CmdError};

const API_ROOT: &str = "https://api.cloudflare.com/client/v4";

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
        /// Connector-local HTTP(S) origin, for example http://localhost:3000.
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

struct CloudflareClient {
    http: reqwest::Client,
    api_token: String,
}

impl CloudflareClient {
    fn new(api_token: String) -> Result<Self, CmdError> {
        if api_token.trim().is_empty() || api_token.chars().any(char::is_whitespace) {
            return Err(CmdError::click(
                "Cloudflare credential field api_token is empty or malformed",
            ));
        }
        Ok(Self {
            http: reqwest::Client::builder().build()?,
            api_token,
        })
    }

    async fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<Value, CmdError> {
        self.send(
            self.http.get(format!("{API_ROOT}{path}")).query(query),
            "GET",
            path,
        )
        .await
    }

    async fn write(&self, method: Method, path: &str, body: &Value) -> Result<Value, CmdError> {
        self.send(
            self.http
                .request(method.clone(), format!("{API_ROOT}{path}"))
                .json(body),
            method.as_str(),
            path,
        )
        .await
    }

    async fn delete(&self, path: &str) -> Result<Value, CmdError> {
        self.send(
            self.http.delete(format!("{API_ROOT}{path}")),
            "DELETE",
            path,
        )
        .await
    }

    async fn send(
        &self,
        request: RequestBuilder,
        method: &str,
        path: &str,
    ) -> Result<Value, CmdError> {
        let response = request.bearer_auth(&self.api_token).send().await?;
        let status = response.status();
        let payload: Value = response.json().await.map_err(|error| {
            CmdError::click(format!(
                "Cloudflare {method} {path} returned unreadable JSON: {error}"
            ))
        })?;
        let success = payload
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if status.is_success() && success {
            return Ok(payload);
        }
        let detail = cloudflare_errors(&payload);
        Err(CmdError::click(format!(
            "Cloudflare {method} {path} failed with HTTP {}: {detail}",
            status.as_u16()
        )))
    }
}

struct TunnelAccess {
    account_id: String,
    tunnel_id: String,
    client: CloudflareClient,
}

impl TunnelAccess {
    fn configuration_path(&self) -> String {
        format!(
            "/accounts/{}/cfd_tunnel/{}/configurations",
            self.account_id, self.tunnel_id
        )
    }

    fn connections_path(&self) -> String {
        format!(
            "/accounts/{}/cfd_tunnel/{}/connections",
            self.account_id, self.tunnel_id
        )
    }

    fn dns_content(&self) -> String {
        format!("{}.cfargotunnel.com", self.tunnel_id)
    }
}

#[derive(Serialize)]
struct RouteInspection {
    hostname: String,
    origin: Option<String>,
    ingress_rules: usize,
    dns_records: usize,
    conflicting_dns_records: usize,
    dns_record_ids: Vec<String>,
    dns_content: String,
    proxied: bool,
    tunnel_connected: bool,
    consistent: bool,
    state: &'static str,
    origin_reachability: &'static str,
}

struct TunnelConnections {
    connector_count: usize,
    active_connections: usize,
}

impl TunnelConnections {
    fn connected(&self) -> bool {
        self.active_connections > 0
    }
}

async fn tunnel_access(
    api_credential_name: &str,
    tunnel_credential_name: &str,
) -> Result<TunnelAccess, CmdError> {
    // Named fields, not whole items: this broker refuses a read that names
    // none. Read-only lifecycle commands never acquire the connector token.
    let account_id = required_field(api_credential_name, "account_id").await?;
    let tunnel_account_id = required_field(tunnel_credential_name, "account_id").await?;
    if account_id != tunnel_account_id {
        return Err(CmdError::click(
            "Cloudflare API and tunnel credentials belong to different accounts",
        ));
    }
    let tunnel_id = required_field(tunnel_credential_name, "tunnel_id").await?;
    let api_token = required_field(api_credential_name, "api_token").await?;
    validate_api_component("account_id", &account_id)?;
    validate_api_component("tunnel_id", &tunnel_id)?;
    Ok(TunnelAccess {
        account_id,
        tunnel_id,
        client: CloudflareClient::new(api_token)?,
    })
}

async fn tunnel_configuration(access: &TunnelAccess) -> Result<Value, CmdError> {
    let current = access.client.get(&access.configuration_path(), &[]).await?;
    Ok(current
        .pointer("/result/config")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new())))
}

async fn active_zone_id(access: &TunnelAccess, zone: &str) -> Result<String, CmdError> {
    let zones = access
        .client
        .get(
            "/zones",
            &[("name", zone), ("status", "active"), ("per_page", "50")],
        )
        .await?;
    exact_zone_id(&zones, zone)
}

async fn tunnel_connections(access: &TunnelAccess) -> Result<TunnelConnections, CmdError> {
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

async fn exact_dns_records(
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

fn is_tunnel_dns_record(record: &Value, expected_content: &str) -> bool {
    record.get("type").and_then(Value::as_str) == Some("CNAME")
        && record
            .get("content")
            .and_then(Value::as_str)
            .is_some_and(|content| content.eq_ignore_ascii_case(expected_content))
}

async fn tunnel_dns_records(access: &TunnelAccess, zone_id: &str) -> Result<Vec<Value>, CmdError> {
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

fn validate_zone_hostname(zone: &str, hostname: &str) -> Result<(), CmdError> {
    validate_dns_name("zone", zone)?;
    validate_dns_name("hostname", hostname)?;
    if !belongs_to_zone(hostname, zone) {
        return Err(CmdError::usage(format!(
            "hostname {hostname:?} is outside zone {zone:?}"
        )));
    }
    Ok(())
}

fn belongs_to_zone(hostname: &str, zone: &str) -> bool {
    hostname == zone
        || hostname
            .strip_suffix(zone)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

fn ingress_rules(config: &Value) -> Result<&[Value], CmdError> {
    if !config.is_object() {
        return Err(CmdError::click(
            "Cloudflare tunnel configuration is not an object",
        ));
    }
    match config.get("ingress") {
        None => Ok(&[]),
        Some(Value::Array(rules)) => Ok(rules),
        Some(_) => Err(CmdError::click(
            "Cloudflare tunnel configuration ingress is not an array",
        )),
    }
}

fn configured_hostnames(config: &Value, zone: &str) -> Result<Vec<String>, CmdError> {
    let mut hostnames: Vec<String> = ingress_rules(config)?
        .iter()
        .filter_map(|rule| rule.get("hostname").and_then(Value::as_str))
        .filter(|hostname| belongs_to_zone(hostname, zone))
        .map(str::to_string)
        .collect();
    hostnames.sort_unstable();
    hostnames.dedup();
    Ok(hostnames)
}

async fn inspect_route(
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

async fn list_routes(
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

async fn route_status(
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

async fn remove_route(
    api_credential_name: &str,
    tunnel_credential_name: &str,
    zone: &str,
    hostname: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    validate_zone_hostname(zone, hostname)?;
    let access = tunnel_access(api_credential_name, tunnel_credential_name).await?;
    let mut config = tunnel_configuration(&access).await?;
    let zone_id = active_zone_id(&access, zone).await?;
    let expected_content = access.dns_content();
    let records = exact_dns_records(&access, &zone_id, hostname).await?;
    let record_ids: Vec<String> = records
        .iter()
        .filter(|record| is_tunnel_dns_record(record, &expected_content))
        .map(|record| required_string(record, "id"))
        .collect::<Result<_, _>>()?;
    let ingress_count = ingress_rules(&config)?
        .iter()
        .filter(|rule| rule.get("hostname").and_then(Value::as_str) == Some(hostname))
        .count();
    if ingress_count == 0 && record_ids.is_empty() {
        return Err(CmdError::click(format!(
            "Cloudflare route {hostname:?} does not exist in tunnel {} or its DNS",
            access.tunnel_id
        )));
    }
    for record_id in &record_ids {
        validate_api_component("record id", record_id)?;
    }
    let removed_ingress_rules = remove_route_ingress(&mut config, hostname)?;

    let records_path = format!("/zones/{zone_id}/dns_records");
    let mut removed_dns_records = 0usize;
    for record_id in &record_ids {
        let path = format!("{records_path}/{record_id}");
        if let Err(error) = access.client.delete(&path).await {
            return Err(CmdError::click(format!(
                "{hostname}: removed {removed_dns_records} tunnel DNS record(s), then Cloudflare refused the next deletion: {error}"
            )));
        }
        removed_dns_records += 1;
    }

    if removed_ingress_rules > 0 {
        if let Err(error) = access
            .client
            .write(
                Method::PUT,
                &access.configuration_path(),
                &json!({ "config": config }),
            )
            .await
        {
            return Err(CmdError::click(format!(
                "{hostname}: removed {removed_dns_records} tunnel DNS record(s), but updating tunnel ingress failed: {error}"
            )));
        }
    }

    let report = json!({
        "status": "removed",
        "api_credential": api_credential_name,
        "tunnel_credential": tunnel_credential_name,
        "account_id": access.account_id,
        "tunnel_id": access.tunnel_id,
        "zone": zone,
        "zone_id": zone_id,
        "hostname": hostname,
        "dns_content": expected_content,
        "removed_dns_records": removed_dns_records,
        "removed_ingress_rules": removed_ingress_rules,
        "connector_preserved": true,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{hostname}: removed {removed_ingress_rules} tunnel ingress rule(s) and {removed_dns_records} tunnel DNS record(s); connector preserved"
        );
    }
    Ok(())
}

// Every parameter is one required field of a tunnel route. Bundling them into a
// struct moves the same list one indirection away without shortening it, and
// this is the release gate's lint, not a design review.
#[allow(clippy::too_many_arguments)]
async fn route_tunnel(
    api_credential_name: &str,
    tunnel_credential_name: &str,
    zone: &str,
    hostname: &str,
    origin: &str,
    host: &str,
    connector_service: &str,
    connector_token_field: &str,
    connector_secret_name: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    validate_zone_hostname(zone, hostname)?;
    validate_origin(origin)?;
    let access = tunnel_access(api_credential_name, tunnel_credential_name).await?;
    let connector_token = required_field(tunnel_credential_name, connector_token_field).await?;

    let declared_services =
        super::service::declared_matching(connector_service, Some(host)).await?;
    let declared = declared_services.first().ok_or_else(|| {
        CmdError::click(format!(
            "service {connector_service:?} is not declared on registry host {host:?}"
        ))
    })?;
    let service_home = managed_service_home(declared)?;
    let configuration_path = access.configuration_path();
    let mut config = tunnel_configuration(&access).await?;
    route_ingress(&mut config, hostname, origin)?;
    access
        .client
        .write(
            Method::PUT,
            &configuration_path,
            &json!({ "config": config }),
        )
        .await?;

    let (connector_secret_path, _) = super::host::install_secret_value_at_home(
        &declared.host,
        connector_secret_name,
        &connector_token,
        &service_home,
    )
    .await?;
    let target = crate::deploy::host_channel::canonical_target(&declared.host)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let restart = crate::deploy::service::restart_service(&target, declared, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !restart.succeeded("restarted") {
        return Err(CmdError::click(format!(
            "{}: connector restart failed: {}",
            declared.host,
            restart.failure()
        )));
    }

    let zone_id = active_zone_id(&access, zone).await?;
    let records_path = format!("/zones/{zone_id}/dns_records");
    let existing = exact_dns_records(&access, &zone_id, hostname).await?;
    if existing.len() > 1 {
        return Err(CmdError::click(format!(
            "Cloudflare returned {} DNS records for exact hostname {hostname:?}; refusing an ambiguous cutover",
            existing.len()
        )));
    }

    let content = access.dns_content();
    let dns_body = json!({
        "type": "CNAME",
        "name": hostname,
        "content": content,
        "ttl": 1,
        "proxied": true,
    });
    let (action, response) = if let Some(record) = existing.first() {
        let record_id = required_string(record, "id")?;
        validate_api_component("record id", &record_id)?;
        let path = format!("{records_path}/{record_id}");
        (
            "updated",
            access.client.write(Method::PUT, &path, &dns_body).await?,
        )
    } else {
        (
            "created",
            access
                .client
                .write(Method::POST, &records_path, &dns_body)
                .await?,
        )
    };
    let record_id = response
        .pointer("/result/id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let report = json!({
        "status": "routed",
        "action": action,
        "api_credential": api_credential_name,
        "tunnel_credential": tunnel_credential_name,
        "account_id": access.account_id,
        "zone": zone,
        "zone_id": zone_id,
        "hostname": hostname,
        "origin": origin,
        "tunnel_id": access.tunnel_id,
        "dns_record_id": record_id,
        "dns_type": "CNAME",
        "dns_content": content,
        "proxied": true,
        "connector_host": declared.host,
        "connector_service": declared.name,
        "connector_unit": declared.unit_id(),
        "connector_secret_path": connector_secret_path,
        "connector_restart": restart.status,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{hostname}: routed through tunnel {} to {origin} ({action} proxied CNAME)",
            access.tunnel_id
        );
    }
    Ok(())
}

fn managed_service_home(
    service: &crate::deploy::service::ManagedService,
) -> Result<String, CmdError> {
    let marker = match service.kind.as_str() {
        crate::deploy::service::KIND_SYSTEMD => "/.config/systemd/user/",
        crate::deploy::service::KIND_LAUNCHD => "/Library/LaunchAgents/",
        other => {
            return Err(CmdError::click(format!(
                "{}: connector service kind {other:?} has no user home contract",
                service.host
            )))
        }
    };
    let (home, _) = service.path.split_once(marker).ok_or_else(|| {
        CmdError::click(format!(
            "{}: connector unit path {:?} must be absolute and identify its service user's home",
            service.host, service.path
        ))
    })?;
    if !home.starts_with('/') || home == "/" {
        return Err(CmdError::click(format!(
            "{}: connector unit path {:?} does not identify a safe service home",
            service.host, service.path
        )));
    }
    Ok(home.to_string())
}

fn route_ingress(config: &mut Value, hostname: &str, origin: &str) -> Result<(), CmdError> {
    if !config.is_object() {
        return Err(CmdError::click(
            "Cloudflare tunnel configuration is not an object",
        ));
    }
    let object = config.as_object_mut().expect("object checked above");
    let ingress = object
        .entry("ingress")
        .or_insert_with(|| Value::Array(Vec::new()));
    let rules = ingress.as_array_mut().ok_or_else(|| {
        CmdError::click("Cloudflare tunnel configuration ingress is not an array")
    })?;

    let matching: Vec<usize> = rules
        .iter()
        .enumerate()
        .filter_map(|(index, rule)| {
            (rule.get("hostname").and_then(Value::as_str) == Some(hostname)).then_some(index)
        })
        .collect();
    if matching.len() > 1 {
        return Err(CmdError::click(format!(
            "Cloudflare tunnel configuration contains duplicate ingress rules for {hostname:?}"
        )));
    }
    let mut route = matching
        .first()
        .map(|index| rules.remove(*index))
        .unwrap_or_else(|| json!({ "hostname": hostname, "originRequest": {} }));
    let route_object = route.as_object_mut().ok_or_else(|| {
        CmdError::click(format!(
            "Cloudflare ingress rule for {hostname:?} is not an object"
        ))
    })?;
    route_object.insert("hostname".to_string(), Value::String(hostname.to_string()));
    route_object.insert("service".to_string(), Value::String(origin.to_string()));

    let fallback_indices: Vec<usize> = rules
        .iter()
        .enumerate()
        .filter_map(|(index, rule)| rule.get("hostname").is_none().then_some(index))
        .collect();
    if fallback_indices.len() > 1 {
        return Err(CmdError::click(
            "Cloudflare tunnel configuration contains more than one catch-all ingress rule",
        ));
    }
    let fallback = fallback_indices
        .first()
        .map(|index| rules.remove(*index))
        .unwrap_or_else(|| json!({ "service": "http_status:404" }));
    rules.push(route);
    rules.push(fallback);
    Ok(())
}

fn remove_route_ingress(config: &mut Value, hostname: &str) -> Result<usize, CmdError> {
    if !config.is_object() {
        return Err(CmdError::click(
            "Cloudflare tunnel configuration is not an object",
        ));
    }
    let object = config.as_object_mut().expect("object checked above");
    let Some(ingress) = object.get_mut("ingress") else {
        return Ok(0);
    };
    let rules = ingress.as_array_mut().ok_or_else(|| {
        CmdError::click("Cloudflare tunnel configuration ingress is not an array")
    })?;
    let before = rules.len();
    rules.retain(|rule| rule.get("hostname").and_then(Value::as_str) != Some(hostname));
    let removed = before - rules.len();
    if removed == 0 {
        return Ok(0);
    }

    let fallback_indices: Vec<usize> = rules
        .iter()
        .enumerate()
        .filter_map(|(index, rule)| rule.get("hostname").is_none().then_some(index))
        .collect();
    if fallback_indices.len() > 1 {
        return Err(CmdError::click(
            "Cloudflare tunnel configuration contains more than one catch-all ingress rule",
        ));
    }
    let fallback = fallback_indices
        .first()
        .map(|index| rules.remove(*index))
        .unwrap_or_else(|| json!({ "service": "http_status:404" }));
    rules.push(fallback);
    Ok(removed)
}

fn exact_zone_id(payload: &Value, zone: &str) -> Result<String, CmdError> {
    let zones = result_array(payload, "Cloudflare zone lookup")?;
    let exact: Vec<&Value> = zones
        .iter()
        .filter(|candidate| candidate.get("name").and_then(Value::as_str) == Some(zone))
        .collect();
    if exact.len() != 1 {
        return Err(CmdError::click(format!(
            "Cloudflare returned {} active exact zones named {zone:?}; expected one",
            exact.len()
        )));
    }
    required_string(exact[0], "id")
}

fn result_array<'a>(payload: &'a Value, context: &str) -> Result<&'a Vec<Value>, CmdError> {
    payload
        .get("result")
        .and_then(Value::as_array)
        .ok_or_else(|| CmdError::click(format!("{context} result is not an array")))
}

fn required_string(value: &Value, field: &str) -> Result<String, CmdError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| CmdError::click(format!("Cloudflare response field {field:?} is required")))
}

/// One required credential field, read by name through the selected store.
async fn required_field(item: &str, field: &str) -> Result<String, CmdError> {
    crate::credential_store::read_string(item, field)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!(
                "credential field {field:?} of {item:?} is required"
            ))
        })
}

fn validate_api_component(label: &str, value: &str) -> Result<(), CmdError> {
    if !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "Cloudflare {label} contains characters that cannot form an API path"
    )))
}

fn validate_dns_name(label: &str, value: &str) -> Result<(), CmdError> {
    let valid = !value.is_empty()
        && value.len() <= 253
        && value == value.to_ascii_lowercase()
        && value.split('.').all(|part| {
            !part.is_empty()
                && part.len() <= 63
                && part.chars().all(|character| {
                    character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
                })
                && !part.starts_with('-')
                && !part.ends_with('-')
        });
    if valid {
        return Ok(());
    }
    Err(CmdError::usage(format!(
        "Cloudflare {label} must be a lowercase DNS name"
    )))
}

fn validate_origin(origin: &str) -> Result<(), CmdError> {
    let parsed = url::Url::parse(origin)
        .map_err(|error| CmdError::usage(format!("Cloudflare origin URL is invalid: {error}")))?;
    if matches!(parsed.scheme(), "http" | "https")
        && parsed.host_str().is_some()
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.fragment().is_none()
    {
        return Ok(());
    }
    Err(CmdError::usage(
        "Cloudflare origin must be an HTTP(S) URL without credentials or a fragment",
    ))
}

fn cloudflare_errors(payload: &Value) -> String {
    let messages: Vec<String> = payload
        .get("errors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|error| error.get("message").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    if messages.is_empty() {
        "Cloudflare returned no error detail".to_string()
    } else {
        messages.join("; ")
    }
}
