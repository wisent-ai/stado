//! The two changing commands: removal deletes matching tunnel DNS before
//! ingress, upsert configures ingress before moving DNS, and both leave the
//! shared connector in place.

use reqwest::Method;
use serde_json::{json, Value};

use super::ingress::{ingress_rules, remove_route_ingress, route_ingress};
use crate::cli::cloudflare::api::{
    required_field, required_string, tunnel_access, validate_api_component, validate_origin,
    validate_zone_hostname,
};
use crate::cli::cloudflare::records::{
    active_zone_id, exact_dns_records, is_tunnel_dns_record, tunnel_configuration,
};
use crate::cli::CmdError;

pub(in crate::cli::cloudflare) async fn remove_route(
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
pub(in crate::cli::cloudflare) async fn route_tunnel(
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
        crate::cli::service::declared_matching(connector_service, Some(host)).await?;
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

    let (connector_secret_path, _) = crate::cli::host::install_secret_value_at_home(
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
        "ttl":
            1,
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
