//! `stado inference route audit [--json]`: every route or fallback that pays
//! an outside provider for a model a running deployment of this fleet
//! already serves.
//!
//! A provider destination is `provider/<model repository>`; it is served by
//! the fleet when a running deployment's `model.repository` is that exact
//! repository. The audit names the deployment and the `route set` that makes
//! it the primary with the outside provider kept as the first fallback; the
//! write stays with `route set`, which holds the alias's current destination
//! as its condition and refuses a destination that is not ready.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::inference::schema;

/// One route or fallback that pays outside for a fleet-served model.
fn finding(
    alias: &str,
    role: &str,
    destination: &str,
    deployment: &schema::Deployment,
    registry: &schema::Registry,
) -> Value {
    let primary = registry.routes.get(alias).map(String::as_str);
    // The fix keeps every destination the alias already falls back to, after
    // the outside one the deployment replaces as primary.
    let mut fallbacks: Vec<&str> = Vec::new();
    match role {
        "primary" => fallbacks.push(destination),
        _ => {
            if let Some(primary) = primary {
                fallbacks.push(primary);
            }
        }
    }
    for kept in registry.fallbacks.get(alias).into_iter().flatten() {
        if kept != &deployment.name && !fallbacks.contains(&kept.as_str()) {
            fallbacks.push(kept);
        }
    }
    let fallback_words: String = fallbacks
        .iter()
        .map(|fallback| format!(" --fallback {fallback}"))
        .collect();
    json!({
        "alias": alias,
        "role": role,
        "destination": destination,
        "deployment": deployment.name,
        "host": deployment.target,
        "repository": deployment.model.repository,
        "revision": deployment.model.revision,
        "fix": format!(
            "stado inference route set {alias} --to {} --expected {}{fallback_words}",
            deployment.name,
            primary.unwrap_or(super::ABSENT)
        ),
    })
}

/// Every route and fallback that pays outside for a model a running
/// deployment serves.
pub(crate) fn findings(registry: &schema::Registry) -> Vec<Value> {
    let served = |destination: &str| -> Option<&schema::Deployment> {
        let (_provider, repository) = destination.split_once('/')?;
        registry.deployments.iter().find(|deployment| {
            deployment.desired_state == schema::STATE_RUNNING
                && deployment.model.repository == repository
        })
    };
    let mut found = Vec::new();
    for (alias, destination) in &registry.routes {
        if let Some(deployment) = served(destination) {
            found.push(finding(alias, "primary", destination, deployment, registry));
        }
    }
    for (alias, fallbacks) in &registry.fallbacks {
        // A fallback to the outside provider behind a fleet primary is the
        // intended shape; only a fallback the fleet serves while the primary
        // is also outside, or not the deployment, is a cost the fleet carries.
        let primary_is_fleet = registry
            .routes
            .get(alias)
            .is_some_and(|primary| registry.deployments.iter().any(|d| &d.name == primary));
        if primary_is_fleet {
            continue;
        }
        for destination in fallbacks {
            if let Some(deployment) = served(destination) {
                found.push(finding(
                    alias,
                    "fallback",
                    destination,
                    deployment,
                    registry,
                ));
            }
        }
    }
    found
}

pub async fn audit(json_output: bool) -> Result<(), CmdError> {
    let document = crate::cli::registry::fetch_document().await?;
    let registry = schema::parse(&document).map_err(CmdError::declaration)?;
    let found = findings(&registry);
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "schema": "stado.inference-route-audit.v1",
                "findings": found,
            }))?
        );
    } else if found.is_empty() {
        println!("no route pays an outside provider for a model a running deployment serves");
    } else {
        for row in &found {
            println!(
                "{} {} {} pays outside for {} served by running deployment {} on {}; to route it \
                 to the fleet with the outside provider as fallback: {}",
                field(&row["alias"]),
                field(&row["role"]),
                field(&row["destination"]),
                field(&row["repository"]),
                field(&row["deployment"]),
                field(&row["host"]),
                field(&row["fix"])
            );
        }
    }
    // A route that pays outside for a model the fleet serves is the finding
    // the audit exists for, so the command exits non-zero on one and a
    // reconcile loop can act on the exit status.
    match found.is_empty() {
        true => Ok(()),
        false => Err(CmdError::silent(crate::cli::CLICK_ERROR_CODE)),
    }
}

fn field(value: &Value) -> &str {
    value.as_str().unwrap_or("?")
}
