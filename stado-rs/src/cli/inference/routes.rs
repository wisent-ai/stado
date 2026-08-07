use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::deploy::{host_channel, inference, inference_routes, production_runner};
use crate::inference::schema;

const ABSENT: &str = "absent";

fn click(error: impl ToString) -> CmdError {
    CmdError::click(error.to_string())
}

fn deployment<'a>(registry: &'a schema::Registry, name: &str) -> Option<&'a schema::Deployment> {
    registry
        .deployments
        .iter()
        .find(|deployment| deployment.name == name)
}

async fn require_ready(registry: &schema::Registry, destination: &str) -> Result<(), CmdError> {
    let Some(deployment) = deployment(registry, destination) else {
        if destination.split_once('/').is_none() {
            return Err(CmdError::click(format!(
                "unknown route destination '{destination}'"
            )));
        }
        return Ok(());
    };
    let bearer = super::credential::read().await?;
    let target = host_channel::canonical_target(&deployment.target)
        .await
        .map_err(click)?;
    let report = inference::probe(&target, deployment, &bearer, &production_runner())
        .await
        .map_err(click)?;
    if report.get("status").and_then(Value::as_str) != Some("ready") {
        return Err(CmdError::click(format!(
            "route destination '{destination}' is not ready"
        )));
    }
    Ok(())
}

fn route_host(registry: &schema::Registry) -> Option<&str> {
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
    require_ready(&registry, to).await?;
    for fallback in fallbacks {
        require_ready(&registry, fallback).await?;
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
    let next = schema::write(&document, &registry).map_err(click)?;
    schema::validate(&next).map_err(click)?;

    let runner = production_runner();
    let mut staged = Value::Null;
    let mut transaction = String::new();
    let target = if let Some(host) = host.as_deref() {
        let target = host_channel::canonical_target(host).await.map_err(click)?;
        transaction = inference_routes::transaction(&registry).map_err(click)?;
        staged = inference_routes::stage(&target, &registry, &transaction, &runner)
            .await
            .map_err(click)?;
        if !inference_routes::ready(&staged, "routes_staged") {
            return Err(CmdError::click("could not stage inference routes"));
        }
        Some(target)
    } else {
        None
    };

    let generation = match crate::cli::registry::push_document_if(&next, &expected_generation).await
    {
        Ok(generation) => generation,
        Err(error) => {
            if let Some(target) = &target {
                let _ = inference_routes::discard(target, &transaction, &runner).await;
            }
            return Err(error);
        }
    };
    let committed = if let Some(target) = &target {
        let result = inference_routes::commit(target, &transaction, &runner).await;
        let committed = result
            .as_ref()
            .is_ok_and(|value| inference_routes::ready(value, "routes_committed"));
        if !committed {
            let rollback = schema::write(&next, &previous_registry).map_err(click)?;
            let registry_rollback =
                crate::cli::registry::push_document_if(&rollback, &generation).await;
            let old_transaction =
                inference_routes::transaction(&previous_registry).map_err(click)?;
            let runtime_rollback =
                if inference_routes::stage(target, &previous_registry, &old_transaction, &runner)
                    .await
                    .is_ok()
                {
                    inference_routes::commit(target, &old_transaction, &runner)
                        .await
                        .is_ok()
                } else {
                    false
                };
            let detail = result
                .err()
                .map(|error| error.to_string())
                .unwrap_or_else(|| "remote commit was refused".to_string());
            if let Err(rollback_error) = registry_rollback {
                return Err(CmdError::click(format!(
                    "route commit failed ({detail}); registry rollback also failed: {rollback_error}"
                )));
            }
            if !runtime_rollback {
                return Err(CmdError::click(format!(
                    "route commit failed ({detail}); registry rolled back but gateway route restoration failed"
                )));
            }
            return Err(CmdError::click(format!(
                "route commit failed ({detail}); registry and gateway route were rolled back"
            )));
        }
        result.map_err(click)?
    } else {
        json!({"status": "not_local"})
    };

    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "generation": generation,
                "alias": alias,
                "from": expected,
                "to": to,
                "fallbacks": fallbacks,
                "runtime": inference_routes::summary(&transaction, staged, committed),
            }))?
        );
    } else {
        println!(
            "route '{alias}': {expected} -> {to} fallbacks={fallbacks:?} generation={generation}"
        );
    }
    Ok(())
}
