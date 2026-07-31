use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::deploy::{host_channel, inference, production_runner};
use crate::inference::{plan as saved_plan, schema};

pub struct PlanOptions {
    pub name: String,
    pub host: String,
    pub image: String,
    pub model: String,
    pub revision: String,
    pub port: u16,
    pub max_model_len: u64,
    pub json: bool,
}

fn click(error: impl ToString) -> CmdError {
    CmdError::click(error.to_string())
}

fn succeeded(value: &Value, expected: &str) -> bool {
    value.get("status").and_then(Value::as_str) == Some(expected)
}
fn field<'a>(report: &'a Value, name: &str) -> Option<&'a str> {
    report
        .get("stdout")
        .and_then(Value::as_str)?
        .lines()
        .find_map(|line| line.split_once('\t').filter(|(key, _)| *key == name))
        .map(|(_, value)| value.trim())
}

fn replace(registry: &mut schema::Registry, deployment: schema::Deployment) {
    registry
        .deployments
        .retain(|current| current.name != deployment.name);
    registry.deployments.push(deployment);
    registry
        .deployments
        .sort_by(|left, right| left.name.cmp(&right.name));
}

async fn wait_ready(
    target: &crate::targets::ComputeTarget,
    deployment: &schema::Deployment,
    bearer: &str,
) -> Result<Value, CmdError> {
    let runner = production_runner();
    let interval = std::time::Duration::from_secs(u64::from(u8::BITS));
    let deadline = tokio::time::Instant::now() + inference::startup_timeout();
    let last = loop {
        let report = inference::probe(target, deployment, bearer, &runner)
            .await
            .map_err(click)?;
        if succeeded(&report, "ready") {
            return Ok(report);
        }
        if report
            .get("stdout")
            .and_then(Value::as_str)
            .is_some_and(|stdout| stdout.contains("inference unit failed"))
        {
            return Err(CmdError::click(format!(
                "inference '{}' unit failed during startup",
                deployment.name
            )));
        }
        if tokio::time::Instant::now() >= deadline {
            break report;
        }
        tokio::time::sleep(interval).await;
    };
    Err(CmdError::click(format!(
        "inference '{}' did not become ready: {}",
        deployment.name, last
    )))
}
async fn restore_after_failed_apply(
    attempted_target: &crate::targets::ComputeTarget,
    attempted: &schema::Deployment,
    runner: &crate::deploy::Runner,
) -> Result<(), CmdError> {
    inference::retire(attempted_target, attempted, false, runner)
        .await
        .map_err(click)?;
    let Some(previous) = attempted.previous.as_deref() else {
        return Ok(());
    };
    let bearer = super::super::service::service_secret(&previous.credential_item, "token").await?;
    let previous_target = host_channel::canonical_target(&previous.target)
        .await
        .map_err(click)?;
    inference::install(&previous_target, previous, &bearer, runner)
        .await
        .map_err(click)?;
    wait_ready(&previous_target, previous, &bearer)
        .await
        .map(|_| ())
}
async fn activate(
    deployment: &schema::Deployment,
    runner: &crate::deploy::Runner,
) -> Result<(), CmdError> {
    let bearer =
        super::super::service::service_secret(&deployment.credential_item, "token").await?;
    let target = host_channel::canonical_target(&deployment.target)
        .await
        .map_err(click)?;
    let installed = inference::install(&target, deployment, &bearer, runner)
        .await
        .map_err(click)?;
    if !succeeded(&installed, "started") {
        return Err(CmdError::click(format!(
            "inference activation failed: {installed}"
        )));
    }
    wait_ready(&target, deployment, &bearer).await.map(|_| ())
}

pub async fn plan(options: PlanOptions) -> Result<(), CmdError> {
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
            && deployment.resources.gpu_mode == schema::GPU_EXCLUSIVE
    }) {
        return Err(CmdError::click(format!(
            "target '{}' already has an exclusive inference deployment",
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
            gpu_mode: schema::GPU_EXCLUSIVE.to_string(),
            gpus: u16::from(true),
            max_model_len: options.max_model_len,
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
    let bearer =
        super::super::service::service_secret(&plan.deployment.credential_item, "token").await?;
    let target = host_channel::canonical_target(&plan.deployment.target)
        .await
        .map_err(click)?;
    let runner = production_runner();
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
    let mut registry = schema::parse(&document).map_err(click)?;
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

pub async fn rollback(name: &str, json_output: bool) -> Result<(), CmdError> {
    let (document, expected_generation) = crate::cli::registry::fetch_versioned_document().await?;
    let mut registry = schema::parse(&document).map_err(click)?;
    let current = registry
        .deployments
        .iter()
        .find(|deployment| deployment.name == name)
        .cloned()
        .ok_or_else(|| CmdError::click(format!("unknown inference deployment '{name}'")))?;
    let previous = current.previous.as_deref().cloned().ok_or_else(|| {
        CmdError::click(format!(
            "inference deployment '{name}' has no rollback generation"
        ))
    })?;
    let runner = production_runner();
    activate(&previous, &runner).await?;
    replace(&mut registry, previous.clone());
    let next = schema::write(&document, &registry).map_err(click)?;
    let generation = match crate::cli::registry::push_document_if(&next, &expected_generation).await
    {
        Ok(generation) => generation,
        Err(error) => {
            if let Err(restore_error) = activate(&current, &runner).await {
                return Err(CmdError::click(format!(
                    "{error}; previous runtime restoration also failed: {restore_error}"
                )));
            }
            return Err(error);
        }
    };
    if current.target != previous.target {
        let current_target = host_channel::canonical_target(&current.target)
            .await
            .map_err(click)?;
        inference::retire(&current_target, &current, false, &runner)
            .await
            .map_err(click)?;
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
    let mut registry = schema::parse(&document).map_err(click)?;
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
        return Err(CmdError::click(format!(
            "route '{alias}' still points at '{name}'"
        )));
    }
    let deployment = registry
        .deployments
        .iter()
        .find(|deployment| deployment.name == name)
        .cloned()
        .ok_or_else(|| CmdError::click(format!("unknown inference deployment '{name}'")))?;
    let target = host_channel::canonical_target(&deployment.target)
        .await
        .map_err(click)?;
    let runner = production_runner();
    let runtime = inference::retire(&target, &deployment, purge_cache, &runner)
        .await
        .map_err(click)?;
    if !succeeded(&runtime, "retired") {
        return Err(CmdError::click(format!(
            "inference retire failed: {runtime}"
        )));
    }
    registry.deployments.retain(|current| current.name != name);
    let next = schema::write(&document, &registry).map_err(click)?;
    let generation = match crate::cli::registry::push_document_if(&next, &expected_generation).await
    {
        Ok(generation) => generation,
        Err(error) => {
            if let Err(restore_error) = activate(&deployment, &runner).await {
                return Err(CmdError::click(format!(
                    "{error}; retired runtime restoration also failed: {restore_error}"
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
