use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const SECTION: &str = "inference";
pub const STATE_RUNNING: &str = "running";
pub const STATE_RETIRED: &str = "retired";
pub const ENGINE_VLLM: &str = "vllm";
pub const GPU_EXCLUSIVE: &str = "exclusive";
pub const VISIBILITY_TAILSCALE: &str = "tailscale";
pub const PROTOCOL_OPENAI_CHAT: &str = "openai-chat";
pub const LOCAL_PROVIDER_CREDENTIAL: &str = "provider:local-openai";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Engine {
    pub name: String,
    pub image: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Model {
    pub repository: String,
    pub revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Resources {
    pub gpu_mode: String,
    pub gpus: u16,
    pub max_model_len: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Endpoint {
    pub host: String,
    pub visibility: String,
    pub port: u16,
    pub protocol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Deployment {
    pub name: String,
    pub target: String,
    pub desired_state: String,
    pub engine: Engine,
    pub model: Model,
    pub resources: Resources,
    pub endpoint: Endpoint,
    pub credential_item: String,
    #[serde(default)]
    pub previous: Option<Box<Deployment>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Registry {
    #[serde(default)]
    pub gateway_target: Option<String>,
    #[serde(default)]
    pub deployments: Vec<Deployment>,
    #[serde(default)]
    pub routes: BTreeMap<String, String>,
    #[serde(default)]
    pub fallbacks: BTreeMap<String, Vec<String>>,
}

fn identifier(value: &str) -> bool {
    let edge = |ch: char| ch.is_ascii_lowercase() || ch.is_ascii_digit();
    let one = usize::from(u8::from(true));
    let two = one.saturating_add(one);
    let maximum = usize::from(u8::MAX).saturating_add(one) / two;
    value.len() <= maximum
        && value.chars().next().is_some_and(edge)
        && value.chars().next_back().is_some_and(edge)
        && value
            .chars()
            .all(|ch| edge(ch) || matches!(ch, '.' | '-' | '_'))
}

fn safe_reference(value: &str, extra: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('/')
        && !value.contains("..")
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "._-".contains(ch) || extra.contains(ch))
}
fn tailscale_ipv4(value: &str) -> bool {
    let Ok(address) = value.parse::<std::net::Ipv4Addr>() else {
        return false;
    };
    let octets = address.octets();
    let first = "100".parse::<u8>().expect("static Tailscale prefix");
    let lower = "64".parse::<u8>().expect("static Tailscale range");
    let upper = "128".parse::<u8>().expect("static Tailscale range");
    octets[usize::MIN] == first && (lower..upper).contains(&octets[usize::from(true)])
}

fn sha256_image(value: &str) -> bool {
    let Some((name, digest)) = value.rsplit_once("@sha256:") else {
        return false;
    };
    let two = usize::from(u8::from(true)).saturating_add(usize::from(u8::from(true)));
    let length = Sha256::output_size().saturating_mul(two);
    safe_reference(name, "/:")
        && digest.len() == length
        && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn immutable_revision(value: &str) -> bool {
    let length = Sha256::output_size().saturating_add(u8::BITS as usize);
    value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

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

pub fn validate(document: &Value) -> Result<(), String> {
    let registry = parse(document)?;
    let targets = document
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| "registry.targets: must be an array".to_string())?;
    let mut names = BTreeSet::new();
    let mut running_names = BTreeSet::new();
    let mut ports = BTreeSet::new();
    let one = u16::from(true);
    let two = one.saturating_add(one);
    let four = two.saturating_add(two);
    let minimum_port = u16::from(u8::MAX).saturating_add(one).saturating_mul(four);
    for deployment in &registry.deployments {
        let location = format!("registry.inference.deployments[{}]", names.len());
        if !identifier(&deployment.name) || !names.insert(deployment.name.as_str()) {
            return Err(format!(
                "{location}.name: must be a unique lowercase identifier"
            ));
        }
        let target = targets.iter().find(|target| {
            target.get("name").and_then(Value::as_str) == Some(deployment.target.as_str())
        });
        let Some(target) = target else {
            return Err(format!(
                "{location}.target: unknown target '{}'",
                deployment.target
            ));
        };
        if target.get("kind").and_then(Value::as_str) != Some("local") {
            return Err(format!(
                "{location}.target: inference requires kind='local'"
            ));
        }
        if target
            .get("vram_gb")
            .and_then(Value::as_u64)
            .unwrap_or(u64::MIN)
            == u64::MIN
        {
            return Err(format!("{location}.target: target declares no GPU VRAM"));
        }
        if deployment.desired_state != STATE_RUNNING && deployment.desired_state != STATE_RETIRED {
            return Err(format!(
                "{location}.desired_state: must be running or retired"
            ));
        }
        if deployment.desired_state == STATE_RUNNING {
            running_names.insert(deployment.name.as_str());
        }
        if deployment.engine.name != ENGINE_VLLM || !sha256_image(&deployment.engine.image) {
            return Err(format!(
                "{location}.engine: vllm image must be pinned by sha256 digest"
            ));
        }
        if !safe_reference(&deployment.model.repository, "/")
            || !immutable_revision(&deployment.model.revision)
        {
            return Err(format!(
                "{location}.model: safe repository and immutable revision are required"
            ));
        }
        if deployment.resources.gpu_mode != GPU_EXCLUSIVE || deployment.resources.gpus != one {
            return Err(format!(
                "{location}.resources: only one exclusive GPU is supported"
            ));
        }
        if deployment.resources.max_model_len == u64::MIN {
            return Err(format!(
                "{location}.resources.max_model_len: must be positive"
            ));
        }
        if deployment.endpoint.visibility != VISIBILITY_TAILSCALE
            || !tailscale_ipv4(&deployment.endpoint.host)
            || deployment.endpoint.protocol != PROTOCOL_OPENAI_CHAT
            || deployment.endpoint.port < minimum_port
            || !ports.insert((deployment.endpoint.host.as_str(), deployment.endpoint.port))
        {
            return Err(format!(
                "{location}.endpoint: requires a unique Tailscale OpenAI chat endpoint"
            ));
        }
        if deployment.credential_item != LOCAL_PROVIDER_CREDENTIAL {
            return Err(format!(
                "{location}.credential_item: must use the central local provider credential"
            ));
        }
    }
    if !registry.routes.is_empty() {
        let gateway = registry.gateway_target.as_deref().ok_or_else(|| {
            "registry.inference.gateway_target is required when routes exist".to_string()
        })?;
        if !targets
            .iter()
            .any(|target| target.get("name").and_then(Value::as_str) == Some(gateway))
        {
            return Err(format!(
                "registry.inference.gateway_target: unknown target '{gateway}'"
            ));
        }
    }
    for (alias, destination) in &registry.routes {
        if alias.split_once('/').is_none() || destination.trim().is_empty() {
            return Err(
                "registry.inference.routes: aliases and destinations must be non-empty routes"
                    .to_string(),
            );
        }
        if !destination.contains('/') && !running_names.contains(destination.as_str()) {
            return Err(format!(
                "registry.inference.routes.{alias}: deployment '{destination}' is not running"
            ));
        }
    }
    for (alias, fallbacks) in &registry.fallbacks {
        let primary = registry.routes.get(alias).ok_or_else(|| {
            format!("registry.inference.fallbacks.{alias}: route has no primary destination")
        })?;
        let mut destinations = BTreeSet::from([primary.as_str()]);
        for destination in fallbacks {
            if destination.trim().is_empty() {
                return Err(format!(
                    "registry.inference.fallbacks.{alias}: destinations must be non-empty"
                ));
            }
            if !destination.contains('/') && !running_names.contains(destination.as_str()) {
                return Err(format!(
                    "registry.inference.fallbacks.{alias}: deployment '{destination}' is not running"
                ));
            }
            if !destinations.insert(destination.as_str()) {
                return Err(format!(
                    "registry.inference.fallbacks.{alias}: duplicate destination '{destination}'"
                ));
            }
        }
    }
    Ok(())
}
