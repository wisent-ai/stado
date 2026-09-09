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

pub(super) fn safe_runtime(deployment: &Deployment) -> Result<(), DeployError> {
    crate::inference::schema::validate(&json!({
        "schema_version": crate::targets::REGISTRY_SCHEMA_VERSION,
        "targets": [{
            "name": deployment.target,
            "kind": "local",
            "gpu_type": "declared",
            "vram_gb": u8::MAX,
        }],
        "inference": {"deployments": [deployment], "routes": {}}
    }))
    .map_err(DeployError)
}

/// Large immutable image pulls and first model loads need a wider bound than
/// ordinary host operations; connection establishment keeps its short SSH cap.
pub fn startup_timeout() -> std::time::Duration {
    host_channel::remote_timeout().saturating_mul(u8::BITS.saturating_mul(u8::BITS))
}
