//! The inference section of the registry: what it declares, what a name in
//! it has to look like, and what the whole section has to satisfy.
//!
//! The shapes are in `shapes`, the name rules in `names`, and the
//! whole-document check in `validate`. What is left here is reading the
//! section and the four mutations an operator makes to it.

use serde_json::Value;

mod names;
mod shapes;
mod validate;

pub use names::gateway_selector;
pub use shapes::{Deployment, Endpoint, Engine, Model, Registry, Resources};
pub use validate::validate;

pub const SECTION: &str = "inference";
pub const STATE_RUNNING: &str = "running";
pub const STATE_RETIRED: &str = "retired";
pub const ENGINE_VLLM: &str = "vllm";
pub const GPU_EXCLUSIVE: &str = "exclusive";
pub const GPU_YIELDABLE: &str = "yieldable";
pub const VISIBILITY_TAILSCALE: &str = "tailscale";
pub const PROTOCOL_OPENAI_CHAT: &str = "openai-chat";
pub const LOCAL_PROVIDER_CREDENTIAL: &str = "provider:local-openai";

pub fn parse(document: &Value) -> Result<Registry, String> {
    let Some(section) = document.get(SECTION) else {
        return Ok(Registry::default());
    };
    serde_json::from_value(section.clone()).map_err(|error| format!("registry.inference: {error}"))
}

pub fn write(document: &Value, registry: &Registry) -> Result<Value, String> {
    let mut next = document.clone();
    let root = next
        .as_object_mut()
        .ok_or_else(|| "registry must be an object".to_string())?;
    root.insert(
        SECTION.to_string(),
        serde_json::to_value(registry).map_err(|error| error.to_string())?,
    );
    Ok(next)
}
pub fn deploy(document: &Value, mut deployment: Deployment) -> Result<Value, String> {
    let mut registry = parse(document)?;
    if let Some(current) = registry
        .deployments
        .iter()
        .find(|current| current.name == deployment.name)
        .cloned()
    {
        let mut previous = current;
        previous.previous = None;
        deployment.previous = Some(Box::new(previous));
        registry
            .deployments
            .retain(|current| current.name != deployment.name);
    }
    registry.deployments.push(deployment);
    let next = write(document, &registry)?;
    validate(&next)?;
    Ok(next)
}

pub fn rollback(document: &Value, name: &str) -> Result<(Value, Option<Deployment>), String> {
    let mut registry = parse(document)?;
    let index = registry
        .deployments
        .iter()
        .position(|deployment| deployment.name == name)
        .ok_or_else(|| format!("inference deployment '{name}' does not exist"))?;
    let current = registry.deployments.remove(index);
    let restored = current.previous.map(|previous| *previous);
    if let Some(previous) = restored.clone() {
        registry.deployments.push(previous);
    } else {
        registry.routes.retain(|_, destination| destination != name);
    }
    let next = write(document, &registry)?;
    validate(&next)?;
    Ok((next, restored))
}

pub fn set_route(
    document: &Value,
    alias: &str,
    destination: &str,
    expected: &str,
) -> Result<Value, String> {
    let mut registry = parse(document)?;
    let current = registry.routes.get(alias).map(String::as_str).unwrap_or("");
    if current != expected {
        return Err(format!(
            "route '{alias}' changed: expected '{expected}', found '{current}'"
        ));
    }
    registry
        .routes
        .insert(alias.to_string(), destination.to_string());
    let next = write(document, &registry)?;
    validate(&next)?;
    Ok(next)
}

pub fn retire(document: &Value, name: &str) -> Result<Value, String> {
    let mut registry = parse(document)?;
    if registry
        .routes
        .values()
        .any(|destination| destination == name)
    {
        return Err(format!(
            "inference deployment '{name}' is still selected by a route"
        ));
    }
    let deployment = registry
        .deployments
        .iter_mut()
        .find(|deployment| deployment.name == name)
        .ok_or_else(|| format!("inference deployment '{name}' does not exist"))?;
    deployment.desired_state = STATE_RETIRED.to_string();
    let next = write(document, &registry)?;
    validate(&next)?;
    Ok(next)
}
