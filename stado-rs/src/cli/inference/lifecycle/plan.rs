//! `stado inference plan` and `apply`: what a deployment would become, and
//! bringing the host to it.

use serde_json::{json, Value};

use super::{
    field, mode_only_change, replace, restore_after_failed_apply, succeeded, wait_ready,
    PlanOptions,
};
use crate::cli::CmdError;
use crate::deploy::{inference, production_runner};
use crate::inference::{plan as saved_plan, schema};

pub async fn plan(options: PlanOptions) -> Result<(), CmdError> {
    let document = crate::cli::registry::fetch_document().await?;
    schema::validate(&document).map_err(CmdError::declaration)?;
    let mut registry = schema::parse(&document).map_err(CmdError::declaration)?;
    let target = crate::cli::canonical_host(&options.host).await?;
    let inventory = inference::inventory(&target, &production_runner())
        .await
        .map_err(CmdError::from)?;
    if !succeeded(&inventory, "inventoried") {
        return Err(
            CmdError::click(format!("target inventory failed: {inventory}"))
                .stating(crate::primitives::failure::FailureCode::InfraDown),
        );
    }
    let addresses = field(&inventory, "ADDRESSES").ok_or_else(|| {
        CmdError::click(format!(
            "the inventory of {} printed no ADDRESSES line, so the host's addresses are unknown: {inventory}",
            options.host
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    let held: Vec<std::net::Ipv4Addr> = addresses
        .split_whitespace()
        .filter_map(|word| word.parse().ok())
        .collect();
    let endpoint_host = held
        .iter()
        .find(|address| options.visibility.holds(**address))
        .ok_or_else(|| {
            let interfaces = match held.is_empty() {
                true => "no global IPv4 address".to_string(),
                false => held
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
            };
            let usable: Vec<&str> = schema::Visibility::ALL
                .into_iter()
                .filter(|other| {
                    *other != options.visibility && held.iter().any(|address| other.holds(*address))
                })
                .map(schema::Visibility::word)
                .collect();
            let fix = match usable.as_slice() {
                [] => format!("connect the host to the {} network", options.visibility),
                usable => format!(
                    "choose --visibility {}, or connect the host to the {} network",
                    usable.join(" or "),
                    options.visibility
                ),
            };
            CmdError::click(format!(
                "{} has no address the {} visibility binds an endpoint on; its interfaces hold \
                 {interfaces}: {fix}",
                options.host, options.visibility
            ))
            .stating(crate::primitives::failure::FailureCode::Refused)
        })?
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
        return Err(CmdError::refused(format!(
            "target '{}' already has a running inference deployment",
            options.host
        )));
    }
    // The board the engine runs on, by the driver's UUID: the one --gpu
    // names, else the board with the most free memory now. Naming it is
    // what lets the host's other boards stay admissible for GPU jobs and
    // rentals while the deployment holds this one.
    let boards_line = field(&inventory, "BOARDS").ok_or_else(|| {
        CmdError::click(format!(
            "the inventory of {} printed no BOARDS line, so its GPU boards are unknown: {inventory}",
            options.host
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    let boards = crate::providers::local::helpers::gpu::inventory::parse_gpu_cards(
        &boards_line.replace(';', "\n"),
    )
    .map_err(|cause| {
        CmdError::click(format!(
            "{}'s GPU boards cannot be read: {cause}",
            options.host
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    let board = match options.gpu.as_deref() {
        Some(asked) => boards
            .iter()
            .find(|board| board.uuid == asked)
            .ok_or_else(|| {
                CmdError::refused(format!(
                    "{} has no GPU board {asked}; its boards are {}",
                    options.host,
                    boards
                        .iter()
                        .map(|board| board.uuid.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })?,
        None => boards
            .iter()
            .max_by_key(|board| board.free_vram_gb)
            .ok_or_else(|| {
                CmdError::refused(format!(
                    "{} reports no GPU board, so no inference engine can be placed on it",
                    options.host
                ))
            })?,
    }
    .uuid
    .clone();
    // What the repository and its base models declare as their license, at
    // the revision the engine loads. A hub that cannot answer refuses the
    // plan: whether the model may be sold is recorded with it, never left
    // unknown.
    let license = crate::inference::license::read(&options.model, &options.revision)
        .await
        .map_err(|cause| {
            CmdError::click(format!(
                "the license of {} cannot be read: {cause}",
                options.model
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?;
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
            license: Some(license.clone()),
        },
        resources: schema::Resources {
            gpu_mode: options.gpu_mode,
            max_model_len: options.max_model_len,
            kv_cache_memory_gb: options.kv_cache_memory_gb,
            cache_dir: options.cache_dir,
            gpu: Some(board),
        },
        endpoint: schema::Endpoint {
            host: endpoint_host,
            visibility: options.visibility.word().to_string(),
            port: options.port,
            protocol: schema::PROTOCOL_OPENAI_CHAT.to_string(),
        },
        credential_item: schema::LOCAL_PROVIDER_CREDENTIAL.to_string(),
        previous: previous.map(Box::new),
    };
    replace(&mut registry, deployment.clone());
    let candidate = schema::write(&document, &registry).map_err(CmdError::declaration)?;
    schema::validate(&candidate).map_err(CmdError::refused)?;

    let plan = saved_plan::create(&document, deployment).map_err(CmdError::from)?;
    let path = saved_plan::save(&plan).map_err(CmdError::from)?;
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
        println!("license={}", license.summary());
        let undeclared = license.undeclared();
        if !undeclared.is_empty() {
            println!(
                "license_warning=no license is declared for {}; serving this model to outside \
                 callers is not permitted until one is",
                undeclared.join(", ")
            );
        }
        if let Some(stdout) = inventory.get("stdout").and_then(Value::as_str) {
            print!("{stdout}");
        }
    }
    Ok(())
}

pub async fn apply(plan_id: &str, json_output: bool) -> Result<(), CmdError> {
    let plan = saved_plan::load(plan_id).map_err(CmdError::from)?;
    let (document, expected_generation) = crate::cli::registry::fetch_versioned_document().await?;
    let actual = saved_plan::document_digest(&document).map_err(CmdError::from)?;
    if actual != plan.expected_registry_sha256 {
        return Err(CmdError::refused(
            "registry changed after inference plan creation; create a new plan",
        ));
    }
    let mut registry = schema::parse(&document).map_err(CmdError::declaration)?;
    let current = registry
        .deployments
        .iter()
        .find(|deployment| deployment.name == plan.deployment.name)
        .cloned();
    let target = crate::cli::canonical_host(&plan.deployment.target).await?;
    let runner = production_runner();

    if let Some(current) = current.filter(|current| mode_only_change(current, &plan.deployment)) {
        let updated = inference::update_reservation(&target, &plan.deployment, &runner)
            .await
            .map_err(CmdError::from)?;
        if succeeded(&updated, "updated") {
            replace(&mut registry, plan.deployment.clone());
            let next = schema::write(&document, &registry).map_err(CmdError::declaration)?;
            let generation =
                match crate::cli::registry::push_document_if(&next, &expected_generation).await {
                    Ok(generation) => generation,
                    Err(error) => {
                        if let Err(restore_error) =
                            inference::update_reservation(&target, &current, &runner).await
                        {
                            return Err(error.also(format_args!(
                                "inference reservation restoration also failed: {restore_error}"
                            )));
                        }
                        return Err(error);
                    }
                };
            saved_plan::consume(plan_id).map_err(CmdError::from)?;
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
                Ok(report) => CmdError::click(format!("inference install failed: {report}"))
                    .stating(crate::primitives::failure::FailureCode::InfraDown),
                Err(error) => CmdError::from(error),
            };
            if let Err(restore_error) =
                restore_after_failed_apply(&target, &plan.deployment, &runner).await
            {
                return Err(install_error.also(format_args!(
                    "runtime restoration also failed: {restore_error}"
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
                return Err(error.also(format_args!(
                    "runtime restoration also failed: {restore_error}"
                )));
            }
            return Err(error);
        }
    };
    replace(&mut registry, plan.deployment.clone());
    let next = schema::write(&document, &registry).map_err(CmdError::declaration)?;
    let generation = match crate::cli::registry::push_document_if(&next, &expected_generation).await
    {
        Ok(generation) => generation,
        Err(error) => {
            if let Err(restore_error) =
                restore_after_failed_apply(&target, &plan.deployment, &runner).await
            {
                return Err(error.also(format_args!(
                    "runtime restoration also failed: {restore_error}"
                )));
            }
            return Err(error);
        }
    };
    saved_plan::consume(plan_id).map_err(CmdError::from)?;
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
