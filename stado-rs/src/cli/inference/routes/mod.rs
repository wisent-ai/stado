use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::deploy::{host_channel, inference, production_runner};
use crate::inference::schema;

pub(super) const ABSENT: &str = "absent";

pub(super) fn click(error: impl ToString) -> CmdError {
    CmdError::click(error.to_string())
}

fn deployment<'a>(registry: &'a schema::Registry, name: &str) -> Option<&'a schema::Deployment> {
    registry
        .deployments
        .iter()
        .find(|deployment| deployment.name == name)
}

pub(super) async fn destination_ready(
    registry: &schema::Registry,
    destination: &str,
) -> Result<bool, CmdError> {
    if schema::gateway_selector(destination) {
        return Ok(true);
    }
    let Some(deployment) = deployment(registry, destination) else {
        if destination.split_once('/').is_none() {
            return Err(CmdError::click(format!(
                "unknown route destination '{destination}'"
            )));
        }
        return Ok(true);
    };
    let bearer = super::credential::read().await?;
    let target = host_channel::canonical_target(&deployment.target)
        .await
        .map_err(click)?;
    let report = inference::probe(&target, deployment, &bearer, &production_runner())
        .await
        .map_err(click)?;
    Ok(report.get("status").and_then(Value::as_str) == Some("ready"))
}

pub(super) fn yieldable_primary(registry: &schema::Registry, destination: &str) -> bool {
    deployment(registry, destination).is_some_and(|deployment| {
        deployment.desired_state == schema::STATE_RUNNING
            && deployment.resources.gpu_mode == schema::GPU_YIELDABLE
    })
}

pub(super) fn route_host(registry: &schema::Registry) -> Option<&str> {
    registry.gateway_target.as_deref()
}

pub async fn set(
    alias: &str,
    to: &str,
    expected: &str,
    gateway: Option<&str>,
    fallbacks: &[String],
    json_output: bool,
) -> Result<(), CmdError> {
    let (document, expected_generation) = crate::cli::registry::fetch_versioned_document().await?;
    let mut registry = schema::parse(&document).map_err(click)?;
    let previous_registry = registry.clone();
    match (registry.gateway_target.as_deref(), gateway) {
        (None, Some(gateway)) => registry.gateway_target = Some(gateway.to_string()),
        (Some(current), Some(gateway)) if current != gateway => {
            return Err(CmdError::click(format!(
                "inference gateway is '{current}', refusing implicit move to '{gateway}'"
            )));
        }
        (None, None) => {
            return Err(CmdError::click(
                "--gateway is required for the first managed inference route",
            ));
        }
        _ => {}
    }
    let current = registry
        .routes
        .get(alias)
        .map(String::as_str)
        .unwrap_or(ABSENT);
    if current != expected {
        return Err(CmdError::click(format!(
            "route '{alias}' is '{current}', expected '{expected}'"
        )));
    }
    if !destination_ready(&registry, to).await?
        && (!yieldable_primary(&registry, to) || fallbacks.is_empty())
    {
        return Err(CmdError::click(format!(
            "route destination '{to}' is not ready"
        )));
    }
    for fallback in fallbacks {
        if !destination_ready(&registry, fallback).await? {
            return Err(CmdError::click(format!(
                "route destination '{fallback}' is not ready"
            )));
        }
    }
    let host = route_host(&registry).map(str::to_string);
    registry.routes.insert(alias.to_string(), to.to_string());
    if fallbacks.is_empty() {
        registry.fallbacks.remove(alias);
    } else {
        registry
            .fallbacks
            .insert(alias.to_string(), fallbacks.to_vec());
    }
    let change = RouteChange {
        report: json!({
            "alias": alias,
            "from": expected,
            "to": to,
            "fallbacks": fallbacks,
        }),
        line: format!("route '{alias}': {expected} -> {to} fallbacks={fallbacks:?}"),
    };
    commit_routes(
        &document,
        &expected_generation,
        host.as_deref(),
        &previous_registry,
        &registry,
        change,
        json_output,
    )
    .await
}

/// Retire one alias from the route table, with the same compare-and-swap
/// precondition `set` demands and the same staged gateway commit behind it.
///
/// An alias that a consumer still asks for must stay until that consumer has
/// moved: the gateway answers an unknown alias with a refusal, not a guess, so
/// removal is a consumer cutover's last step and never its first.
pub async fn remove(alias: &str, expected: &str, json_output: bool) -> Result<(), CmdError> {
    let (document, expected_generation) = crate::cli::registry::fetch_versioned_document().await?;
    let mut registry = schema::parse(&document).map_err(click)?;
    let previous_registry = registry.clone();
    let Some(current) = registry.routes.get(alias).map(String::as_str) else {
        return Err(CmdError::click(format!(
            "route '{alias}' is '{ABSENT}'; nothing to remove"
        )));
    };
    if current != expected {
        return Err(CmdError::click(format!(
            "route '{alias}' is '{current}', expected '{expected}'"
        )));
    }
    let host = route_host(&registry).map(str::to_string);
    registry.routes.remove(alias);
    registry.fallbacks.remove(alias);
    let change = RouteChange {
        report: json!({
            "alias": alias,
            "from": expected,
            "to": ABSENT,
            "fallbacks": [],
        }),
        line: format!("route '{alias}': {expected} -> {ABSENT}"),
    };
    commit_routes(
        &document,
        &expected_generation,
        host.as_deref(),
        &previous_registry,
        &registry,
        change,
        json_output,
    )
    .await
}

mod commit;
mod show;

pub use show::show;

use commit::{commit_routes, RouteChange};
