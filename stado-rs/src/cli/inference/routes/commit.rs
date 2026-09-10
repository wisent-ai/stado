//! The one transaction both route mutations ride: validate, stage the table
//! on the gateway host, compare-and-swap the registry, then commit — rolling
//! both back together when the gateway refuses, so the registry never
//! declares a route the gateway does not serve.

use serde_json::{json, Value};

use super::click;
use crate::cli::CmdError;
use crate::deploy::{host_channel, inference::routes, production_runner};
use crate::inference::schema;

/// What one route mutation reports once the registry and the gateway agree.
pub(super) struct RouteChange {
    pub(super) report: Value,
    pub(super) line: String,
}

/// Validate the mutated registry, stage its route table on the gateway host,
/// compare-and-swap the registry, then commit the staged table — rolling both
/// back together when the gateway refuses, so the registry never declares a
/// route the gateway does not serve.
pub(super) async fn commit_routes(
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
        transaction = routes::transaction(registry).map_err(click)?;
        staged = routes::stage(&target, registry, &transaction, &runner)
            .await
            .map_err(click)?;
        if !routes::ready(&staged, "routes_staged") {
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
                let _ = routes::discard(target, &transaction, &runner).await;
            }
            return Err(error);
        }
    };
    let committed = if let Some(target) = &target {
        let result = routes::commit(target, &transaction, &runner).await;
        let committed = result
            .as_ref()
            .is_ok_and(|value| routes::ready(value, "routes_committed"));
        if !committed {
            let rollback = schema::write(&next, previous_registry).map_err(click)?;
            let registry_rollback =
                crate::cli::registry::push_document_if(&rollback, &generation).await;
            let old_transaction =
                routes::transaction(previous_registry).map_err(click)?;
            let runtime_rollback =
                if routes::stage(target, previous_registry, &old_transaction, &runner)
                    .await
                    .is_ok()
                {
                    routes::commit(target, &old_transaction, &runner)
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
        report["runtime"] = routes::summary(&transaction, staged, committed);
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{} generation={generation}", change.line);
    }
    Ok(())
}
