//! The one transaction both route mutations ride: validate, stage the table
//! on the gateway host, compare-and-swap the registry, then commit — rolling
//! both back together when the gateway refuses, so the registry never
//! declares a route the gateway does not serve.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::deploy::{inference::routes, production_runner};
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
    let next = schema::write(document, registry).map_err(CmdError::declaration)?;
    schema::validate(&next).map_err(CmdError::refused)?;

    let runner = production_runner();
    let mut staged = Value::Null;
    let mut transaction = String::new();
    let target = if let Some(host) = host {
        let target = crate::cli::canonical_host(host).await?;
        transaction = routes::transaction(registry).map_err(CmdError::from)?;
        staged = routes::stage(&target, registry, &transaction, &runner)
            .await
            .map_err(CmdError::from)?;
        if !routes::ready(&staged, "routes_staged") {
            return Err(CmdError::click("could not stage inference routes")
                .stating(crate::primitives::failure::FailureCode::InfraDown));
        }
        Some(target)
    } else {
        None
    };

    let generation = match crate::cli::registry::push_document_if(&next, expected_generation).await
    {
        Ok(generation) => generation,
        Err(error) => {
            // A staged transaction left on the host is said, so it is not
            // mistaken for one a later commit made.
            if let Some(target) = &target {
                if let Err(discard) = routes::discard(target, &transaction, &runner).await {
                    return Err(error.also(format!(
                        "the staged route transaction {transaction} is left on the host: {discard}"
                    )));
                }
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
            let rollback =
                schema::write(&next, previous_registry).map_err(CmdError::declaration)?;
            let registry_rollback =
                crate::cli::registry::push_document_if(&rollback, &generation).await;
            let old_transaction = routes::transaction(previous_registry).map_err(CmdError::from)?;
            // The old transaction is staged and committed again; whichever
            // step failed is named, so a route left half-restored says why.
            let runtime_rollback =
                match routes::stage(target, previous_registry, &old_transaction, &runner).await {
                    Ok(_staged) => match routes::commit(target, &old_transaction, &runner).await {
                        Ok(_committed) => Ok(()),
                        Err(error) => Err(format!(
                            "committing the previous routes again failed: {error}"
                        )),
                    },
                    Err(error) => Err(format!("staging the previous routes again failed: {error}")),
                };
            // The commit's own answer: its error, or the document the gateway
            // returned without `routes_committed`, quoted so the refusal is
            // the gateway's and not a sentence of ours.
            let detail = match result {
                Err(error) => error.to_string(),
                Ok(answer) => format!("the gateway answered without routes_committed: {answer}"),
            };
            if let Err(rollback_error) = registry_rollback {
                return Err(CmdError::click(format!(
                    "route commit failed ({detail}); registry rollback also failed: {rollback_error}"
                ))
                .stating(crate::primitives::failure::FailureCode::InfraDown));
            }
            if let Err(restoration) = runtime_rollback {
                return Err(CmdError::click(format!(
                    "route commit failed ({detail}); registry rolled back but gateway route restoration failed: {restoration}"
                ))
                .stating(crate::primitives::failure::FailureCode::InfraDown));
            }
            return Err(CmdError::click(format!(
                "route commit failed ({detail}); registry and gateway route were rolled back"
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown));
        }
        result.map_err(CmdError::from)?
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
