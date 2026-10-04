//! Taking a deployment back out: rolling one to its predecessor, retiring
//! one for good, and aborting a plan that was never applied.

use serde_json::json;

use super::{activate, replace, succeeded};
use crate::cli::CmdError;
use crate::deploy::{inference, production_runner};
use crate::inference::{plan as saved_plan, schema};

pub async fn rollback(name: &str, json_output: bool) -> Result<(), CmdError> {
    let (document, expected_generation) = crate::cli::registry::fetch_versioned_document().await?;
    let mut registry = schema::parse(&document).map_err(CmdError::declaration)?;
    let current = registry
        .deployments
        .iter()
        .find(|deployment| deployment.name == name)
        .cloned()
        .ok_or_else(|| CmdError::refused(format!("unknown inference deployment '{name}'")))?;
    let previous = current.previous.as_deref().cloned().ok_or_else(|| {
        CmdError::refused(format!(
            "inference deployment '{name}' has no rollback generation"
        ))
    })?;
    let runner = production_runner();
    activate(&previous, &runner).await?;
    replace(&mut registry, previous.clone());
    let next = schema::write(&document, &registry).map_err(CmdError::declaration)?;
    let generation = match crate::cli::registry::push_document_if(&next, &expected_generation).await
    {
        Ok(generation) => generation,
        Err(error) => {
            if let Err(restore_error) = activate(&current, &runner).await {
                return Err(error.also(format_args!(
                    "previous runtime restoration also failed: {restore_error}"
                )));
            }
            return Err(error);
        }
    };
    if current.target != previous.target {
        let current_target = crate::cli::canonical_host(&current.target).await?;
        inference::retire(&current_target, &current, false, &runner)
            .await
            .map_err(CmdError::from)?;
    }
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"generation": generation, "deployment": previous})
            )?
        );
    } else {
        println!("rolled back '{name}' generation={generation}");
    }
    Ok(())
}

pub async fn retire(name: &str, purge_cache: bool, json_output: bool) -> Result<(), CmdError> {
    let (document, expected_generation) = crate::cli::registry::fetch_versioned_document().await?;
    let mut registry = schema::parse(&document).map_err(CmdError::declaration)?;
    if let Some(alias) = registry
        .routes
        .iter()
        .find_map(|(alias, destination)| (destination == name).then_some(alias))
        .or_else(|| {
            registry.fallbacks.iter().find_map(|(alias, destinations)| {
                destinations
                    .iter()
                    .any(|destination| destination == name)
                    .then_some(alias)
            })
        })
    {
        return Err(CmdError::refused(format!(
            "route '{alias}' still points at '{name}'"
        )));
    }
    let deployment = registry
        .deployments
        .iter()
        .find(|deployment| deployment.name == name)
        .cloned()
        .ok_or_else(|| CmdError::refused(format!("unknown inference deployment '{name}'")))?;
    let target = crate::cli::canonical_host(&deployment.target).await?;
    let runner = production_runner();
    let runtime = inference::retire(&target, &deployment, purge_cache, &runner)
        .await
        .map_err(CmdError::from)?;
    if !succeeded(&runtime, "retired") {
        return Err(
            CmdError::click(format!("inference retire failed: {runtime}"))
                .stating(crate::primitives::failure::FailureCode::InfraDown),
        );
    }
    registry.deployments.retain(|current| current.name != name);
    let next = schema::write(&document, &registry).map_err(CmdError::declaration)?;
    let generation = match crate::cli::registry::push_document_if(&next, &expected_generation).await
    {
        Ok(generation) => generation,
        Err(error) => {
            if let Err(restore_error) = activate(&deployment, &runner).await {
                return Err(error.also(format_args!(
                    "retired runtime restoration also failed: {restore_error}"
                )));
            }
            return Err(error);
        }
    };
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"generation": generation, "runtime": runtime}))?
        );
    } else {
        println!("retired '{name}' generation={generation}");
    }
    Ok(())
}

pub async fn abort(plan_id: &str, purge_cache: bool, json_output: bool) -> Result<(), CmdError> {
    let plan = saved_plan::load(plan_id).map_err(CmdError::from)?;
    let target = crate::cli::canonical_host(&plan.deployment.target).await?;
    let runtime = inference::retire(&target, &plan.deployment, purge_cache, &production_runner())
        .await
        .map_err(CmdError::from)?;
    if !succeeded(&runtime, "retired") {
        return Err(
            CmdError::click(format!("inference plan abort failed: {runtime}"))
                .stating(crate::primitives::failure::FailureCode::InfraDown),
        );
    }
    saved_plan::consume(plan_id).map_err(CmdError::from)?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"plan_id": plan_id, "runtime": runtime}))?
        );
    } else {
        println!("aborted inference plan {plan_id}");
    }
    Ok(())
}
