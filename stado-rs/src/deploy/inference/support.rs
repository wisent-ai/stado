//! Report shaping, unit naming, runtime validation and the wider startup bound.

use serde_json::{json, Value};

use crate::deploy::{host_channel, CommandOutput, DeployError};
use crate::inference::schema::Deployment;
use crate::targets::ComputeTarget;

pub(super) fn report(target: &ComputeTarget, output: &CommandOutput, ok: &str) -> Value {
    let mut body = host_channel::base_report(target);
    host_channel::finish_report(&mut body, output, ok, "inference operation failed");
    body.insert("stdout".to_string(), Value::String(output.stdout.clone()));
    Value::Object(body)
}

pub(super) fn unit_name(name: &str) -> String {
    format!("stado-inference-{name}.service")
}

/// The model cache directory as a shell word for the deployment's scripts:
/// the declared `resources.cache_dir`, quoted, or `cache` under the
/// deployment's own root (`$root`, which the scripts set first) when the
/// deployment declares none.
pub(super) fn cache_dir_word(deployment: &Deployment) -> String {
    match deployment.resources.cache_dir.as_deref() {
        Some(declared) => crate::deploy::shlex_quote(declared),
        None => "\"$root/cache\"".to_string(),
    }
}

pub(super) fn safe_runtime(deployment: &Deployment) -> Result<(), DeployError> {
    crate::inference::schema::validate(&json!({
        "schema_version": crate::targets::registry_schema_version(),
        "targets": [{
            "name": deployment.target,
            "kind": "local",
            "gpu_type": "declared",
            "vram_gb": u8::MAX,
        }],
        "inference": {"deployments": [deployment], "routes": {}}
    }))
    .map_err(|message| {
        DeployError(message).stating(crate::primitives::failure::FailureCode::Config)
    })
}
