//! Service directory reads shared by every `stado route` operation.

use serde::Serialize;
use serde_json::{Map, Value};

use crate::cli::CmdError;
use crate::deploy::host_channel;
use crate::targets::{self, ComputeTarget, Registry};

const DIRECTORY: &str = "service_directory";
const SERVICES: &str = "service_directory.services";

const UNKNOWN_SERVICE_SUFFIX: &str =
    "is not in the service directory; add it to service_directory.services";
const NO_AUTHORITY: &str =
    "the service directory declares no authority; add it to service_directory.authority";

#[derive(Debug, Clone, Serialize)]
pub struct Authority {
    pub target: String,
    pub command: String,
}

pub struct DirectoryView<'a> {
    pub authority: Authority,
    pub services: &'a Map<String, Value>,
}

pub struct ServiceView<'a> {
    pub name: &'a str,
    pub active_host: &'a str,
    pub endpoints: Option<&'a Map<String, Value>>,
}

pub fn directory(document: &Value) -> Result<DirectoryView<'_>, CmdError> {
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

pub fn service<'a>(
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

pub fn selected_target<'a>(service: &ServiceView<'a>, target: Option<&'a str>) -> &'a str {
    target.unwrap_or(service.active_host)
}

pub fn endpoint<'a>(service: &'a ServiceView<'a>, target: &str) -> Result<&'a str, CmdError> {
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

pub fn parsed_registry(document: &Value) -> Result<Registry, CmdError> {
    targets::load_registry_from_str(&serde_json::to_string(document)?)
        .map_err(|error| CmdError::click(error.to_string()))
}

pub fn target<'a>(registry: &'a Registry, name: &str) -> Result<&'a ComputeTarget, CmdError> {
    host_channel::resolve_target(registry, name).map_err(|error| CmdError::click(error.to_string()))
}
