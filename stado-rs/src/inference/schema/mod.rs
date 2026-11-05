//! The inference section of the registry: what it declares, what a name in
//! it has to look like, and what the whole section has to satisfy.
//!
//! The shapes are in `shapes`, the name rules in `names`, and the
//! whole-document check in `validate`. What is left here is reading the
//! section and the four mutations an operator makes to it.

use serde::{Deserialize, Serialize};
use serde_json::Value;

mod names;
mod shapes;
mod validate;

pub use names::gateway_selector;
pub use shapes::{DeclaredEngine, Deployment, Endpoint, Engine, Kind, Model, Registry, Resources};
pub use validate::validate;

pub const SECTION: &str = "inference";
pub const STATE_RUNNING: &str = "running";
pub const STATE_RETIRED: &str = "retired";
pub const ENGINE_VLLM: &str = "vllm";

/// How an inference deployment holds its GPU, as the registry and the
/// host's reservation name it; another word is refused where it is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
#[value(rename_all = "lowercase")]
pub enum GpuMode {
    /// The GPU stays reserved for inference: no GPU claim is admitted.
    Exclusive,
    /// Inference pauses whenever an eligible GPU job is queued and resumes
    /// afterward.
    Yieldable,
}

impl std::fmt::Display for GpuMode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        crate::primitives::word::write(self, formatter)
    }
}

/// The network a deployment's endpoint is bound on, as the registry names
/// it. The endpoint is a vLLM listener, so it needs an address of the host's
/// own: Stado's network carries SSH between hosts and has no address to bind,
/// so the host's own private network is the path that needs no outside
/// provider, and Tailscale is one an operator may choose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
#[value(rename_all = "lowercase")]
pub enum Visibility {
    /// A private IPv4 address of one of the host's own interfaces
    /// (10/8, 172.16/12, 192.168/16).
    Lan,
    /// The host's Tailscale IPv4 address (100.64.0.0/10).
    Tailscale,
}

impl Visibility {
    /// Every visibility, as a refusal names them.
    pub const ALL: [Self; 2] = [Self::Lan, Self::Tailscale];

    pub fn word(self) -> &'static str {
        match self {
            Self::Lan => "lan",
            Self::Tailscale => "tailscale",
        }
    }

    /// The visibility the registry writes as `word`, if it is one.
    pub fn named(word: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|visibility| visibility.word() == word)
    }

    /// Whether `address` is one this visibility binds an endpoint on.
    pub fn holds(self, address: std::net::Ipv4Addr) -> bool {
        let [first, second, ..] = address.octets();
        let shared = first == 100 && (64..128).contains(&second);
        match self {
            Self::Lan => address.is_private(),
            Self::Tailscale => shared,
        }
    }
}

impl std::fmt::Display for Visibility {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.word())
    }
}

pub const PROTOCOL_OPENAI_CHAT: &str = "openai-chat";
/// The protocol of a deployment whose declared engine answers its own
/// request shape, named by the engine's declaration.
pub const PROTOCOL_DECLARED: &str = "declared";
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
    let current = registry.routes.get(alias).map(String::as_str);
    let unchanged = match current {
        Some(found) => found == expected,
        None => expected.is_empty(),
    };
    if !unchanged {
        let found = match current {
            Some(found) => format!("'{found}'"),
            None => "no route".to_string(),
        };
        return Err(format!(
            "route '{alias}' changed: expected '{expected}', found {found}"
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
