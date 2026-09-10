//! `stado inference plan` and `apply`: what a deployment would become, and
//! bringing the host to it.

use serde_json::{json, Value};

use super::{
    click, field, mode_only_change, replace, restore_after_failed_apply, succeeded, wait_ready,
    PlanOptions,
};
use crate::cli::CmdError;
use crate::deploy::{host_channel, inference, production_runner};
use crate::inference::{plan as saved_plan, schema};

pub async fn plan(options: PlanOptions) -> Result<(), CmdError> {
    if !matches!(
        options.gpu_mode.as_str(),
        schema::GPU_EXCLUSIVE | schema::GPU_YIELDABLE
    ) {
        return Err(CmdError::click(
            "gpu mode must be 'exclusive' or 'yieldable'",
        ));
    }
    let document = crate::cli::registry::fetch_document().await?;
    schema::validate(&document).map_err(click)?;
    let mut registry = schema::parse(&document).map_err(click)?;
    let target = host_channel::canonical_target(&options.host)
        .await
        .map_err(click)?;
    let inventory = inference::inventory(&target, &production_runner())
        .await
        .map_err(click)?;
    if !succeeded(&inventory, "inventoried") {
        return Err(CmdError::click(format!(
            "target inventory failed: {inventory}"
        )));
    }
    let endpoint_host = field(&inventory, "TAILSCALE")
        .ok_or_else(|| CmdError::click("target inventory returned no Tailscale IPv4 address"))?
        .to_string();
    let previous = registry
        .deployments
        .iter()
        .find(|deployment| deployment.name == options.name)
        .cloned()
        .map(|mut deployment| {
            deployment.previous = None;
            deployment
        });
    if registry.deployments.iter().any(|deployment| {
        deployment.name != options.name
            && deployment.target == options.host
            && deployment.desired_state == schema::STATE_RUNNING
    }) {
        return Err(CmdError::click(format!(
            "target '{}' already has a running inference deployment",
            options.host
        )));
    }
    let deployment = schema::Deployment {
        name: options.name,
        target: options.host,
        desired_state: schema::STATE_RUNNING.to_string(),
        engine: schema::Engine {
            name: schema::ENGINE_VLLM.to_string(),
            image: options.image,
        },
        model: schema::Model {
            repository: options.model,
            revision: options.revision,
        },
        resources: schema::Resources {
            gpu_mode: options.gpu_mode,
            gpus: u16::from(true),
            max_model_len: options.max_model_len,
            kv_cache_memory_gb: options.kv_cache_memory_gb,
            cache_dir: options.cache_dir,
        },
        endpoint: schema::Endpoint {
            host: endpoint_host,
            visibility: schema::VISIBILITY_TAILSCALE.to_string(),
            port: options.port,
            protocol: schema::PROTOCOL_OPENAI_CHAT.to_string(),
        },
        credential_item: schema::LOCAL_PROVIDER_CREDENTIAL.to_string(),
        previous: previous.map(Box::new),
    };
    replace(&mut registry, deployment.clone());
    let candidate = schema::write(&document, &registry).map_err(click)?;
    schema::validate(&candidate).map_err(click)?;

    let plan = saved_plan::create(&document, deployment).map_err(click)?;
    let path = saved_plan::save(&plan).map_err(click)?;
    if options.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "plan": plan,
                "plan_file": path,
                "inventory": inventory,
            }))?
        );
    } else {
        println!("plan_id={} file={}", plan.id, path.display());
        if let Some(stdout) = inventory.get("stdout").and_then(Value::as_str) {
            print!("{stdout}");
        }
    }
    Ok(())
}

pub async fn apply(plan_id: &str, json_output: bool) -> Result<(), CmdError> {
    let plan = saved_plan::load(plan_id).map_err(click)?;
    let (document, expected_generation) = crate::cli::registry::fetch_versioned_document().await?;
    let actual = saved_plan::document_digest(&document).map_err(click)?;
    if actual != plan.expected_registry_sha256 {
        return Err(CmdError::click(
            "registry changed after inference plan creation; create a new plan",
        ));
    }
    let mut registry = schema::parse(&document).map_err(click)?;
    let current = registry
        .deployments
        .iter()
        .find(|deployment| deployment.name == plan.deployment.name)
        .cloned();
    let target = host_channel::canonical_target(&plan.deployment.target)
        .await
        .map_err(click)?;
    let runner = production_runner();

    if let Some(current) = current.filter(|current| mode_only_change(current, &plan.deployment)) {
        let updated = inference::update_reservation(&target, &plan.deployment, &runner)
            .await
            .map_err(click)?;
        if succeeded(&updated, "updated") {
            replace(&mut registry, plan.deployment.clone());
            let next = schema::write(&document, &registry).map_err(click)?;
            let generation = match crate::cli::registry::push_document_if(
                &next,
                &expected_generation,
            )
            .await
            {
                Ok(generation) => generation,
                Err(error) => {
                    if let Err(restore_error) =
                        inference::update_reservation(&target, &current, &runner).await
                    {
                        return Err(CmdError::click(format!(
                                "{error}; inference reservation restoration also failed: {restore_error}"
                            )));
                    }
                    return Err(error);
                }
            };
            saved_plan::consume(plan_id).map_err(click)?;
            if json_output {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&json!({
                        "generation": generation,
                        "deployment": plan.deployment,
                        "runtime": updated,
                        "ready": {"status": "unchanged"},
                    }))?
                );
            } else {
                println!("applied inference plan {plan_id} generation={generation}");
            }
            return Ok(());
        }
    }

    let bearer = crate::cli::inference::credential::read().await?;
    let installed = match inference::install(&target, &plan.deployment, &bearer, &runner).await {
        Ok(installed) if succeeded(&installed, "started") => installed,
        result => {
            let install_error = match result {
                Ok(report) => CmdError::click(format!("inference install failed: {report}")),
                Err(error) => click(error),
            };
            if let Err(restore_error) =
                restore_after_failed_apply(&target, &plan.deployment, &runner).await
            {
                return Err(CmdError::click(format!(
                    "{install_error}; runtime restoration also failed: {restore_error}"
                )));
            }
            return Err(install_error);
        }
    };
    let ready = match wait_ready(&target, &plan.deployment, &bearer).await {
        Ok(report) => report,
        Err(error) => {
            if let Err(restore_error) =
                restore_after_failed_apply(&target, &plan.deployment, &runner).await
            {
                return Err(CmdError::click(format!(
                    "{error}; runtime restoration also failed: {restore_error}"
                )));
            }
            return Err(error);
        }
    };
    replace(&mut registry, plan.deployment.clone());
    let next = schema::write(&document, &registry).map_err(click)?;
    let generation = match crate::cli::registry::push_document_if(&next, &expected_generation).await
    {
        Ok(generation) => generation,
        Err(error) => {
            if let Err(restore_error) =
                restore_after_failed_apply(&target, &plan.deployment, &runner).await
            {
                return Err(CmdError::click(format!(
                    "{error}; runtime restoration also failed: {restore_error}"
                )));
            }
            return Err(error);
        }
    };
    saved_plan::consume(plan_id).map_err(click)?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "generation": generation,
                "deployment": plan.deployment,
                "runtime": installed,
                "ready": ready,
            }))?
        );
    } else {
        println!("applied inference plan {plan_id} generation={generation}");
    }
    Ok(())
}
