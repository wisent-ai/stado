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

async fn destination_ready(
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

fn yieldable_primary(registry: &schema::Registry, destination: &str) -> bool {
    deployment(registry, destination).is_some_and(|deployment| {
        deployment.desired_state == schema::STATE_RUNNING
            && deployment.resources.gpu_mode == schema::GPU_YIELDABLE
    })
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

/// One alias, as the registry declares it and as the gateway host serves it.
fn entry(registry: &schema::Registry, alias: &str) -> Value {
    json!({
        "destination": registry.routes.get(alias),
        "fallbacks": registry.fallbacks.get(alias).cloned().unwrap_or_default(),
    })
}

/// Compare the declared route table with the one the gateway process reads,
/// and optionally restage the declaration onto the host.
///
/// [`set`] and [`remove`] write both sides in one transaction, so they cannot
/// disagree. Every other writer of the canonical registry moves the
/// declaration alone: the gateway keeps serving the table it was last staged,
/// and no command said which of the two an operator was looking at. This is
/// that command. `--repair` sends the declaration to the host through the same
/// stage-and-commit the mutations use; the registry is never rewritten from
/// the host, because placement is declared, not observed.
pub async fn show(repair: bool, json_output: bool) -> Result<(), CmdError> {
    let document = crate::cli::registry::fetch_document().await?;
    let registry = schema::parse(&document).map_err(click)?;
    let Some(host) = route_host(&registry) else {
        return Err(CmdError::click(
            "registry.inference declares no gateway target, so no host serves a route table",
        ));
    };
    let runner = production_runner();
    let target = host_channel::canonical_target(host).await.map_err(click)?;
    let live = inference_routes::live(&target, &runner)
        .await
        .map_err(click)?;
    // `stage` writes the serialized registry SECTION, not a whole registry
    // document, so the host's table has `routes` at its top level and
    // `schema::parse` — which reads `document["inference"]` — would report every
    // alias as absent rather than say it could not find the section.
    let served = match live.as_ref() {
        Some(value) => Some(
            serde_json::from_value::<schema::Registry>(value.clone())
                .map_err(|error| click(format!("the gateway route table is invalid: {error}")))?,
        ),
        None => None,
    };
    let mut aliases = registry
        .routes
        .keys()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    if let Some(served) = &served {
        aliases.extend(served.routes.keys().cloned());
    }
    let mut rows = Vec::new();
    let mut diverged = Vec::new();
    for alias in &aliases {
        let declared = entry(&registry, alias);
        let serving = served
            .as_ref()
            .map(|served| entry(served, alias))
            .unwrap_or(Value::Null);
        let agrees = serving == declared;
        if !agrees {
            diverged.push(alias.clone());
        }
        rows.push(json!({
            "alias": alias,
            "declared": declared,
            "serving": serving,
            "agrees": agrees,
        }));
    }
    let repaired = if repair && !diverged.is_empty() {
        let transaction = inference_routes::transaction(&registry).map_err(click)?;
        let staged = inference_routes::stage(&target, &registry, &transaction, &runner)
            .await
            .map_err(click)?;
        if !inference_routes::ready(&staged, "routes_staged") {
            return Err(CmdError::click("could not stage inference routes"));
        }
        let committed = inference_routes::commit(&target, &transaction, &runner)
            .await
            .map_err(click)?;
        if !inference_routes::ready(&committed, "routes_committed") {
            return Err(CmdError::click(
                "the gateway refused the declared route table",
            ));
        }
        Some(inference_routes::summary(&transaction, staged, committed))
    } else {
        None
    };
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "gateway": target.name,
                "serving_table": if live.is_some() { "present" } else { ABSENT },
                "aliases": rows,
                "diverged": diverged,
                "repair": repaired,
            }))?
        );
    } else {
        println!(
            "gateway {}: route table {}",
            target.name,
            if live.is_some() { "present" } else { ABSENT }
        );
        for row in &rows {
            let alias = row["alias"].as_str().unwrap_or_default();
            let verdict = if row["agrees"] == json!(true) {
                "agrees"
            } else {
                "DIVERGED"
            };
            println!(
                "{alias:<32} {verdict:<9} declared={} serving={}",
                row["declared"], row["serving"]
            );
        }
        if repaired.is_some() {
            println!(
                "restaged the declared table on {}: {} alias(es) repaired",
                target.name,
                diverged.len()
            );
        }
    }
    if !diverged.is_empty() && repaired.is_none() {
        return Err(CmdError::click(format!(
            "the gateway on {} does not serve the declared route table for {}; \
             re-run with --repair to stage and commit the declaration",
            target.name,
            diverged.join(",")
        )));
    }
    Ok(())
}

/// What one route mutation reports once the registry and the gateway agree.
struct RouteChange {
    report: Value,
    line: String,
}

/// Validate the mutated registry, stage its route table on the gateway host,
/// compare-and-swap the registry, then commit the staged table — rolling both
/// back together when the gateway refuses, so the registry never declares a
/// route the gateway does not serve.
async fn commit_routes(
    document: &Value,
    expected_generation: &str,
    host: Option<&str>,
    previous_registry: &schema::Registry,
    registry: &schema::Registry,
    change: RouteChange,
    json_output: bool,
) -> Result<(), CmdError> {
    let next = schema::write(document, registry).map_err(click)?;
    schema::validate(&next).map_err(click)?;

    let runner = production_runner();
    let mut staged = Value::Null;
    let mut transaction = String::new();
    let target = if let Some(host) = host {
        let target = host_channel::canonical_target(host).await.map_err(click)?;
        transaction = inference_routes::transaction(registry).map_err(click)?;
        staged = inference_routes::stage(&target, registry, &transaction, &runner)
            .await
            .map_err(click)?;
        if !inference_routes::ready(&staged, "routes_staged") {
            return Err(CmdError::click("could not stage inference routes"));
        }
        Some(target)
    } else {
        None
    };

    let generation = match crate::cli::registry::push_document_if(&next, expected_generation).await
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
            let rollback = schema::write(&next, previous_registry).map_err(click)?;
            let registry_rollback =
                crate::cli::registry::push_document_if(&rollback, &generation).await;
            let old_transaction =
                inference_routes::transaction(previous_registry).map_err(click)?;
            let runtime_rollback =
                if inference_routes::stage(target, previous_registry, &old_transaction, &runner)
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
        let mut report = change.report;
        report["generation"] = json!(generation);
        report["runtime"] = inference_routes::summary(&transaction, staged, committed);
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{} generation={generation}", change.line);
    }
    Ok(())
}
