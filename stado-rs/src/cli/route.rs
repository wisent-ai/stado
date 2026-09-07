//! `stado route` — routing operations derived from the service directory.
//!
//! A service name resolves once, through `service_directory.services`; the
//! command never carries a product-to-host or product-to-port table of its own.

use std::collections::BTreeSet;

use clap::Subcommand;
use serde::Serialize;
use serde_json::{json, Map, Value};

use super::{placement, registry, CmdError};
use crate::deploy::{host_capability, host_channel, host_forward, host_resolver_key};
use crate::targets::{self, ComputeTarget, Registry};

const DIRECTORY: &str = "service_directory";
const SERVICES: &str = "service_directory.services";

const UNKNOWN_SERVICE_SUFFIX: &str =
    "is not in the service directory; add it to service_directory.services";
const NO_AUTHORITY: &str =
    "the service directory declares no authority; add it to service_directory.authority";

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

#[derive(Debug, Clone, Serialize)]
struct Authority {
    target: String,
    command: String,
}

struct DirectoryView<'a> {
    authority: Authority,
    services: &'a Map<String, Value>,
}

struct ServiceView<'a> {
    name: &'a str,
    active_host: &'a str,
    endpoints: Option<&'a Map<String, Value>>,
}

fn directory(document: &Value) -> Result<DirectoryView<'_>, CmdError> {
    let directory = document
        .get(DIRECTORY)
        .and_then(Value::as_object)
        .ok_or_else(|| {
            CmdError::click(
                "the registry declares no service directory; add service_directory to registry.json",
            )
        })?;
    let authority = directory
        .get("authority")
        .and_then(Value::as_object)
        .and_then(|authority| {
            let target = authority.get("target")?.as_str()?.trim();
            let command = authority.get("command")?.as_str()?.trim();
            if target.is_empty() || command.is_empty() {
                return None;
            }
            Some(Authority {
                target: target.to_string(),
                command: command.to_string(),
            })
        })
        .ok_or_else(|| CmdError::click(NO_AUTHORITY))?;
    let services = directory
        .get("services")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            CmdError::click(
                "the service directory declares no services; add them to service_directory.services",
            )
        })?;
    Ok(DirectoryView {
        authority,
        services,
    })
}

fn service<'a>(
    directory: &'a DirectoryView<'a>,
    name: &'a str,
) -> Result<ServiceView<'a>, CmdError> {
    let entry = directory
        .services
        .get(name)
        .and_then(Value::as_object)
        .ok_or_else(|| CmdError::click(format!("{name} {UNKNOWN_SERVICE_SUFFIX}")))?;
    let active_host = entry
        .get("active_host")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!(
                "{name} declares no active host; add it to {SERVICES}.{name}.active_host"
            ))
        })?;
    let endpoints = entry.get("endpoints").and_then(Value::as_object);
    Ok(ServiceView {
        name,
        active_host,
        endpoints,
    })
}

fn selected_target<'a>(service: &ServiceView<'a>, target: Option<&'a str>) -> &'a str {
    target.unwrap_or(service.active_host)
}

fn endpoint<'a>(service: &'a ServiceView<'a>, target: &str) -> Result<&'a str, CmdError> {
    service
        .endpoints
        .and_then(|endpoints| endpoints.get(target))
        .and_then(|endpoint| endpoint.get("url"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!(
                "{} declares no endpoint for {target}; add it to {}.{}.endpoints.{target}",
                service.name, SERVICES, service.name
            ))
        })
}

fn parsed_registry(document: &Value) -> Result<Registry, CmdError> {
    targets::load_registry_from_str(&serde_json::to_string(document)?)
        .map_err(|error| CmdError::click(error.to_string()))
}

fn target<'a>(registry: &'a Registry, name: &str) -> Result<&'a ComputeTarget, CmdError> {
    host_channel::resolve_target(registry, name).map_err(|error| CmdError::click(error.to_string()))
}

pub async fn dispatch(command: RouteCommands) -> Result<(), CmdError> {
    match command {
        RouteCommands::List { json } => list(json).await,
        RouteCommands::Open {
            service,
            target,
            local,
            remote,
            json,
        } => open(&service, target.as_deref(), local, remote, json).await,
        RouteCommands::Close { service, target } => close(&service, target.as_deref()).await,
        RouteCommands::Capability { service, json } => capability(&service, json).await,
        RouteCommands::Key { target, json } => key(&target, json).await,
        RouteCommands::Placement(RoutePlacementCommands::Publish { mobile, json }) => {
            publish_placement(mobile, json).await
        }
    }
}

async fn list(as_json: bool) -> Result<(), CmdError> {
    let document = registry::fetch_document().await?;
    let directory = directory(&document)?;
    let mut rows = Vec::with_capacity(directory.services.len());
    for name in directory.services.keys() {
        let declared = service(&directory, name)?;
        let endpoints = declared
            .endpoints
            .into_iter()
            .flat_map(|endpoints| endpoints.iter())
            .map(|(target, endpoint)| {
                json!({
                    "target": target,
                    "url": endpoint.get("url").and_then(Value::as_str),
                })
            })
            .collect::<Vec<_>>();
        let open =
            host_forward::read_local(name).map_err(|error| CmdError::click(error.to_string()))?;
        rows.push(json!({
            "service": name,
            "authority": &directory.authority,
            "active_host": declared.active_host,
            "endpoints": endpoints,
            "local_forward": open,
        }));
    }
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "authority": directory.authority,
                "services": rows,
            }))?
        );
        return Ok(());
    }
    for row in rows {
        println!(
            "{}  authority={}  command={:?}  active={}",
            row["service"].as_str().unwrap_or_default(),
            row["authority"]["target"].as_str().unwrap_or_default(),
            row["authority"]["command"].as_str().unwrap_or_default(),
            row["active_host"].as_str().unwrap_or_default(),
        );
        for endpoint in row["endpoints"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            println!(
                "  {} -> {}",
                endpoint["target"].as_str().unwrap_or_default(),
                endpoint["url"].as_str().unwrap_or("(not declared)"),
            );
        }
        match &row["local_forward"] {
            Value::Null => println!("  local forward: closed"),
            marker => println!(
                "  local forward: open -> {} ({})",
                marker["url"].as_str().unwrap_or_default(),
                marker["marker"].as_str().unwrap_or_default(),
            ),
        }
    }
    Ok(())
}

async fn open(
    name: &str,
    requested_target: Option<&str>,
    local: bool,
    remote: bool,
    as_json: bool,
) -> Result<(), CmdError> {
    if !local && !remote {
        return Err(CmdError::usage(
            "route open requires --local or --remote; choose where the declared forward marker must live",
        ));
    }
    let document = registry::fetch_document().await?;
    let directory = directory(&document)?;
    let declared = service(&directory, name)?;
    let target_name = selected_target(&declared, requested_target);
    let url = endpoint(&declared, target_name)?;
    let marker = if local {
        host_forward::open_local(name, url).map_err(|error| CmdError::click(error.to_string()))?
    } else {
        let registry = parsed_registry(&document)?;
        let target = target(&registry, target_name)?;
        host_forward::open_remote(target, name, url)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?
    };
    let report = json!({
        "service": name,
        "authority": directory.authority,
        "active_host": declared.active_host,
        "endpoint": url,
        "forward": marker,
        "status": "open",
    });
    if as_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{}: open on {} -> {} ({})",
            name,
            report["forward"]["location"].as_str().unwrap_or_default(),
            url,
            report["forward"]["marker"].as_str().unwrap_or_default(),
        );
    }
    Ok(())
}

async fn close(name: &str, requested_target: Option<&str>) -> Result<(), CmdError> {
    let document = registry::fetch_document().await?;
    let directory = directory(&document)?;
    let declared = service(&directory, name)?;
    let target_name = selected_target(&declared, requested_target);
    endpoint(&declared, target_name)?;

    let local_removed =
        host_forward::close_local(name).map_err(|error| CmdError::click(error.to_string()))?;
    let mut remote_removed = false;
    if !local_removed {
        let registry = parsed_registry(&document)?;
        let target = target(&registry, target_name)?;
        if !host_channel::target_is_this_host(target) {
            remote_removed = host_forward::close_remote(target, name)
                .await
                .map_err(|error| CmdError::click(error.to_string()))?;
        }
    }
    if !local_removed && !remote_removed {
        return Err(CmdError::click(format!(
            "{name} has no open forward marker; run `stado route open {name} --local` or `stado route open {name} --remote` first"
        )));
    }
    println!(
        "{name}: closed {} forward; no marker remains",
        if local_removed { "local" } else { "remote" }
    );
    Ok(())
}

async fn capability(name: &str, as_json: bool) -> Result<(), CmdError> {
    let document = registry::fetch_document().await?;
    let directory = directory(&document)?;
    let declared = service(&directory, name)?;
    let registry = parsed_registry(&document)?;
    let target = target(&registry, declared.active_host)?;
    let runner = crate::deploy::production_runner();
    let broker =
        host_capability::resolve(target, &host_capability::BrokerFiles::default(), &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
    let routes = host_capability::routes(target, &broker, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let report = json!({
        "service": name,
        "authority": directory.authority,
        "active_host": declared.active_host,
        "vault": broker.vault,
        "report": routes,
    });
    if as_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("service:   {name}");
        println!("host:      {}", declared.active_host);
        println!("authority: {}", directory.authority.target);
        println!("vault:     {}", broker.vault);
        let rows = report["report"]["routes"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        println!("routes:    {}", rows.len());
        for row in rows {
            println!(
                "  {:<52} {}/{}",
                row["resource"].as_str().unwrap_or_default(),
                row["item"].as_str().unwrap_or_default(),
                row["field"].as_str().unwrap_or_default(),
            );
        }
    }
    Ok(())
}

async fn key(target: &str, as_json: bool) -> Result<(), CmdError> {
    let document = registry::fetch_document().await?;
    let directory = directory(&document)?;
    let parsed = parsed_registry(&document)?;
    let report = host_resolver_key::authorize(&parsed, target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "service_directory_authority": directory.authority,
                "resolver_key": report,
            }))?
        );
    } else {
        println!(
            "{}: resolver key {} ({}), authorized_keys on {} {}",
            report["target"].as_str().unwrap_or_default(),
            report["key_state"].as_str().unwrap_or_default(),
            report["key_type"].as_str().unwrap_or_default(),
            report["authority"].as_str().unwrap_or_default(),
            report["authorized_keys"].as_str().unwrap_or_default(),
        );
    }
    Ok(())
}

async fn publish_placement(mobile: bool, as_json: bool) -> Result<(), CmdError> {
    let (document, generation) = registry::fetch_versioned_document().await?;
    let directory = directory(&document)?;
    let registry = parsed_registry(&document)?;
    let mut hosts: BTreeSet<&str> = BTreeSet::new();
    for name in directory.services.keys() {
        hosts.insert(service(&directory, name)?.active_host);
    }
    if mobile {
        hosts.retain(|name| {
            registry
                .lookup(name)
                .is_some_and(|target| target.mobile_runtime.is_some())
        });
    }
    if hosts.is_empty() {
        return Err(CmdError::click(if mobile {
            "the service directory has no active host declaring mobile_runtime; add it to the serving target declaration"
        } else {
            "the service directory declares no active hosts; add active_host to service_directory.services entries"
        }));
    }

    let mut published = Vec::with_capacity(hosts.len());
    for host in hosts {
        published
            .push(placement::publish_placement_policy_report(&document, &generation, host).await?);
    }
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "authority": directory.authority,
                "mobile": mobile,
                "published": published,
            }))?
        );
    } else {
        for report in &published {
            println!(
                "{}: published {} actions at registry generation {}",
                report["target"].as_str().unwrap_or_default(),
                report["actions"].as_array().map_or(0, Vec::len),
                report["registry_generation"].as_str().unwrap_or_default(),
            );
        }
    }
    Ok(())
}
