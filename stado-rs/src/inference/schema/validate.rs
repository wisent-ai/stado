//! Everything the inference section has to satisfy read together: each
//! deployment against its target, each route against a deployment that
//! exists, and every alias against the purpose its model declares.

use std::collections::BTreeSet;

use serde_json::Value;

use super::names::{
    gateway_selector, identifier, immutable_revision, route_alias, safe_reference, sha256_image,
};
use super::{
    parse, Visibility, ENGINE_VLLM, LOCAL_PROVIDER_CREDENTIAL, PROTOCOL_OPENAI_CHAT, STATE_RETIRED,
    STATE_RUNNING,
};

pub fn validate(document: &Value) -> Result<(), String> {
    let registry = parse(document)?;
    let targets = document
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| "registry.targets: must be an array".to_string())?;
    let mut names = BTreeSet::new();
    let mut running_names = BTreeSet::new();
    let mut ports = BTreeSet::new();
    let mut boards = BTreeSet::new();
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
        let Some(target_vram_gb) = target
            .get("vram_gb")
            .and_then(Value::as_u64)
            .and_then(std::num::NonZeroU64::new)
            .map(std::num::NonZeroU64::get)
        else {
            return Err(format!("{location}.target: target declares no GPU VRAM"));
        };
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
        if std::num::NonZeroU64::new(deployment.resources.max_model_len).is_none() {
            return Err(format!(
                "{location}.resources.max_model_len: must be positive"
            ));
        }
        if deployment
            .resources
            .kv_cache_memory_gb
            .is_some_and(|value| {
                std::num::NonZeroU64::new(value).is_none() || value > target_vram_gb
            })
        {
            return Err(format!(
                "{location}.resources.kv_cache_memory_gb: must be positive and at most the target's {target_vram_gb} GiB VRAM"
            ));
        }
        if deployment
            .resources
            .cache_dir
            .as_deref()
            .is_some_and(|path| {
                !path
                    .strip_prefix('/')
                    .is_some_and(|relative| safe_reference(relative, "/"))
            })
        {
            return Err(format!(
                "{location}.resources.cache_dir: must be a safe absolute path"
            ));
        }
        if let Some(gpu) = deployment.resources.gpu.as_deref() {
            // The driver's own UUID form, `GPU-` and hex groups: it is
            // passed to Docker's device request and matched against the
            // driver's reading, so anything else could name no board.
            let uuid = gpu.strip_prefix("GPU-").is_some_and(|rest| {
                !rest.is_empty() && rest.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
            });
            if !uuid {
                return Err(format!(
                    "{location}.resources.gpu: {gpu:?} is not a GPU UUID as nvidia-smi prints \
                     it (GPU-<hex>)"
                ));
            }
            if deployment.desired_state == STATE_RUNNING
                && !boards.insert((deployment.target.as_str(), gpu))
            {
                return Err(format!(
                    "{location}.resources.gpu: {gpu} on {} is already another running \
                     deployment's board",
                    deployment.target
                ));
            }
        }
        let endpoint = &deployment.endpoint;
        let visibility = Visibility::named(&endpoint.visibility).ok_or_else(|| {
            format!(
                "{location}.endpoint.visibility: {:?} is no visibility; it is one of {}",
                endpoint.visibility,
                Visibility::ALL.map(Visibility::word).join(", ")
            )
        })?;
        match endpoint.host.parse::<std::net::Ipv4Addr>() {
            Ok(address) if visibility.holds(address) => {}
            Ok(_) => {
                return Err(format!(
                    "{location}.endpoint.host: {} is not an address the {visibility} visibility binds on",
                    endpoint.host
                ))
            }
            Err(_) => {
                return Err(format!(
                    "{location}.endpoint.host: {:?} is not an IPv4 address",
                    endpoint.host
                ))
            }
        }
        if endpoint.protocol != PROTOCOL_OPENAI_CHAT {
            return Err(format!(
                "{location}.endpoint.protocol: must be {PROTOCOL_OPENAI_CHAT}"
            ));
        }
        if !ports.insert((endpoint.host.as_str(), endpoint.port)) {
            return Err(format!(
                "{location}.endpoint: {}:{} is already another deployment's endpoint",
                endpoint.host, endpoint.port
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
        if !route_alias(alias) || destination.trim().is_empty() {
            return Err(format!(
                "registry.inference.routes: alias '{alias}' must be lowercase identifiers joined by '/', and its destination must be non-empty"
            ));
        }
        if !destination.contains('/')
            && !gateway_selector(destination)
            && !running_names.contains(destination.as_str())
        {
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
            if !destination.contains('/')
                && !gateway_selector(destination)
                && !running_names.contains(destination.as_str())
            {
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
    for (repository, purpose) in &registry.model_purposes {
        if !safe_reference(repository, "/") || !identifier(purpose) {
            return Err(format!(
                "registry.inference.model_purposes.{repository}: purpose must be a lowercase identifier for a safe model repository"
            ));
        }
    }
    for (alias, purpose) in &registry.alias_purposes {
        if !route_alias(alias) || !identifier(purpose) {
            return Err(format!(
                "registry.inference.alias_purposes.{alias}: purpose must be a lowercase identifier for a route alias"
            ));
        }
        if !registry.routes.contains_key(alias) {
            return Err(format!(
                "registry.inference.alias_purposes.{alias}: no route declares this alias"
            ));
        }
    }
    // A destination names a model either directly (`provider/repo`) or through
    // a named deployment. When that model carries a declared purpose, the only
    // aliases allowed to select it are the ones living under that purpose
    // (`<purpose>/...`) — a general-purpose alias silently serving a
    // special-purpose model is exactly the binding this refuses.
    let destination_purpose = |destination: &str| -> Option<&str> {
        let repository = match destination.split_once('/') {
            Some((_, repository)) => repository,
            None => registry
                .deployments
                .iter()
                .find(|deployment| deployment.name == destination)
                .map(|deployment| deployment.model.repository.as_str())?,
        };
        registry
            .model_purposes
            .get(repository)
            .or_else(|| registry.model_purposes.get(destination))
            .map(String::as_str)
    };
    let alias_bindings = registry
        .routes
        .iter()
        .map(|(alias, destination)| (alias, destination, "routes"))
        .chain(registry.fallbacks.iter().flat_map(|(alias, fallbacks)| {
            fallbacks
                .iter()
                .map(move |destination| (alias, destination, "fallbacks"))
        }));
    for (alias, destination, table) in alias_bindings {
        let Some(purpose) = destination_purpose(destination) else {
            continue;
        };
        // The alias's declared purpose, else the one its own name carries. An
        // agent alias declares none, so it keeps the namespace rule and keeps
        // being refused.
        let namespace = match registry.alias_purposes.get(alias) {
            Some(declared) => declared.as_str(),
            // The part before the first slash; an alias without one is its
            // own namespace.
            None => match alias.split_once('/') {
                Some((namespace, _)) => namespace,
                None => alias.as_str(),
            },
        };
        if namespace != purpose {
            return Err(format!(
                "registry.inference.{table}.{alias}: model '{destination}' is declared \
                 {purpose}-only and may only serve aliases under '{purpose}/' or an alias \
                 declared {purpose} in registry.inference.alias_purposes"
            ));
        }
    }
    Ok(())
}
