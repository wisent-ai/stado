use serde_json::Value;

use crate::cli::CmdError;
use crate::deploy::{host_channel, inference, production_runner};
use crate::inference::schema;

pub struct PlanOptions {
    pub name: String,
    pub host: String,
    pub image: String,
    pub model: String,
    pub revision: String,
    pub gpu_mode: String,
    pub port: u16,
    pub max_model_len: u64,
    pub kv_cache_memory_gb: Option<u64>,
    pub cache_dir: Option<String>,
    pub json: bool,
}

pub(super) fn click(error: impl ToString) -> CmdError {
    CmdError::click(error.to_string())
}

pub(super) fn succeeded(value: &Value, expected: &str) -> bool {
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

pub(super) fn replace(registry: &mut schema::Registry, deployment: schema::Deployment) {
    registry
        .deployments
        .retain(|current| current.name != deployment.name);
    registry.deployments.push(deployment);
    registry
        .deployments
        .sort_by(|left, right| left.name.cmp(&right.name));
}

pub(super) async fn wait_ready(
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
            .is_some_and(|stdout| {
                stdout.contains("inference container is")
                    || stdout.contains("inference container missing")
            })
        {
            return Err(CmdError::click(format!(
                "inference '{}' container failed during startup",
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
pub(super) async fn restore_after_failed_apply(
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
    let bearer = super::credential::read().await?;
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
pub(super) async fn activate(
    deployment: &schema::Deployment,
    runner: &crate::deploy::Runner,
) -> Result<(), CmdError> {
    let bearer = super::credential::read().await?;
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

mod plan;
mod retire;

pub use plan::{apply, plan};
pub use retire::{abort, retire, rollback};

pub(super) fn mode_only_change(
    current: &schema::Deployment,
    candidate: &schema::Deployment,
) -> bool {
    current.resources.gpu_mode != candidate.resources.gpu_mode
        && current.name == candidate.name
        && current.target == candidate.target
        && current.desired_state == candidate.desired_state
        && current.engine == candidate.engine
        && current.model == candidate.model
        && current.resources.gpus == candidate.resources.gpus
        && current.resources.max_model_len == candidate.resources.max_model_len
        && current.resources.kv_cache_memory_gb == candidate.resources.kv_cache_memory_gb
        && current.resources.cache_dir == candidate.resources.cache_dir
        && current.endpoint == candidate.endpoint
        && current.credential_item == candidate.credential_item
}
