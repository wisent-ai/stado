//! Native Cloudflare tunnel routing through credentials held by Stado.
//!
//! The command updates a tunnel ingress before moving DNS, so a failed ingress
//! write can never point public traffic at an unconfigured connector. Secret
//! values stay in memory and are never rendered in command output.

use clap::Subcommand;
use reqwest::{Method, RequestBuilder};
use serde_json::{json, Map, Value};

use super::CmdError;

const API_ROOT: &str = "https://api.cloudflare.com/client/v4";

#[derive(Subcommand)]
pub enum CloudflareCommands {
    /// Route one hostname to an origin behind a named Cloudflare Tunnel.
    #[command(name = "route-tunnel")]
    RouteTunnel {
        /// Stado credential containing the Cloudflare account_id and scoped api_token.
        #[arg(long)]
        api_credential: String,
        /// Stado credential containing the same account_id, tunnel_id and connector token.
        #[arg(long)]
        tunnel_credential: String,
        /// Exact Cloudflare zone name, for example bobloo.com.
        #[arg(long)]
        zone: String,
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
}

pub async fn dispatch(command: CloudflareCommands) -> Result<(), CmdError> {
    match command {
        CloudflareCommands::RouteTunnel {
            api_credential,
            tunnel_credential,
            zone,
            hostname,
            origin,
            host,
            connector_service,
            connector_token_field,
            connector_secret_name,
            json,
        } => {
            route_tunnel(
                &api_credential,
                &tunnel_credential,
                &zone,
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
    validate_dns_name("zone", zone)?;
    validate_dns_name("hostname", hostname)?;
    if hostname != zone && !hostname.ends_with(&format!(".{zone}")) {
        return Err(CmdError::usage(format!(
            "hostname {hostname:?} is outside zone {zone:?}"
        )));
    }
    validate_origin(origin)?;

    // Named fields, not whole items: this broker refuses a read that names
    // none, so the whole-item form failed before any Cloudflare call was made.
    let account_id = required_field(api_credential_name, "account_id").await?;
    let tunnel_account_id = required_field(tunnel_credential_name, "account_id").await?;
    if account_id != tunnel_account_id {
        return Err(CmdError::click(
            "Cloudflare API and tunnel credentials belong to different accounts",
        ));
    }
    let tunnel_id = required_field(tunnel_credential_name, "tunnel_id").await?;
    let api_token = required_field(api_credential_name, "api_token").await?;
    let connector_token = required_field(tunnel_credential_name, connector_token_field).await?;
    validate_api_component("account_id", &account_id)?;
    validate_api_component("tunnel_id", &tunnel_id)?;
    let client = CloudflareClient::new(api_token)?;

    let declared_services =
        super::service::declared_matching(connector_service, Some(host)).await?;
    let declared = declared_services.first().ok_or_else(|| {
        CmdError::click(format!(
            "service {connector_service:?} is not declared on registry host {host:?}"
        ))
    })?;
    let service_home = managed_service_home(declared)?;
    let configuration_path =
        format!("/accounts/{account_id}/cfd_tunnel/{tunnel_id}/configurations");
    let current = client.get(&configuration_path, &[]).await?;
    let mut config = current
        .pointer("/result/config")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    route_ingress(&mut config, hostname, origin)?;
    client
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

    let zones = client
        .get(
            "/zones",
            &[("name", zone), ("status", "active"), ("per_page", "50")],
        )
        .await?;
    let zone_id = exact_zone_id(&zones, zone)?;
    let records_path = format!("/zones/{zone_id}/dns_records");
    let records = client
        .get(&records_path, &[("name", hostname), ("per_page", "100")])
        .await?;
    let existing = result_array(&records, "Cloudflare DNS record lookup")?;
    if existing.len() > 1 {
        return Err(CmdError::click(format!(
            "Cloudflare returned {} DNS records for exact hostname {hostname:?}; refusing an ambiguous cutover",
            existing.len()
        )));
    }

    let content = format!("{tunnel_id}.cfargotunnel.com");
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
            client.write(Method::PUT, &path, &dns_body).await?,
        )
    } else {
        (
            "created",
            client.write(Method::POST, &records_path, &dns_body).await?,
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
        "account_id": account_id,
        "zone": zone,
        "zone_id": zone_id,
        "hostname": hostname,
        "origin": origin,
        "tunnel_id": tunnel_id,
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
            "{hostname}: routed through tunnel {tunnel_id} to {origin} ({action} proxied CNAME)"
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
        .ok_or_else(|| CmdError::click(format!("credential field {field:?} is required")))
}

/// One required credential field, read by name through the selected store.
async fn required_field(item: &str, field: &str) -> Result<String, CmdError> {
    crate::credential_store::read_string(item, field)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!("credential field {field:?} of {item:?} is required"))
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
