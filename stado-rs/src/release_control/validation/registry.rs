//! What the registry's `release_control` document must say before it is
//! accepted.

use std::collections::BTreeSet;
use std::net::SocketAddr;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde_json::Value;

use crate::release::canonical_coordinate;
use crate::release_control::documents::policy::valid_legacy_launchd_unit;
use crate::release_control::{control, StrategyKind, DEFAULT_REPLACE_READINESS_PATH};

use super::shape::{env_name, identifier, safe_absolute, safe_install_root, safe_relative, sha256};

pub fn validate_registry_contract(document: &Value) -> Result<(), String> {
    let Some(control) = control(document)? else {
        return Ok(());
    };
    if control.schema_version != 1 {
        return Err("registry.release_control.schema_version must be 1".to_string());
    }
    if control.generation == 0 {
        return Err("registry.release_control.generation must be positive".to_string());
    }
    // Trust and desired state are separate declarations, and a fleet may hold
    // the first without the second: a leased scratch target is delivered
    // releases on request and reconciles nothing, and a fleet before its first
    // product is in the same position. What must never pass is a block no
    // consumer reads, which is the pair being empty -- `products` alone was
    // the wrong test for that, and it is why `stado scratch` could not be
    // delivered a signed release at all: the emitted registry could carry no
    // trust keys without also inventing products and services the lease does
    // not have, so `host-state --apply` refused every pipeline-signed version
    // with `registry declares no release trust keys`.
    if control.products.is_empty() && control.trusted_keys.is_empty() {
        return Err("registry.release_control must declare trusted_keys or products".to_string());
    }
    for (key_id, public_key) in &control.trusted_keys {
        if !identifier(key_id)
            || BASE64
                .decode(public_key)
                .ok()
                .filter(|bytes| bytes.len() == 32)
                .is_none()
        {
            return Err(format!(
                "registry.release_control.trusted_keys.{key_id} is not an Ed25519 public key"
            ));
        }
    }
    let targets = document
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| "registry.targets must be an array".to_string())?;
    let target_names: BTreeSet<_> = targets
        .iter()
        .filter_map(|target| target.get("name").and_then(Value::as_str))
        .collect();
    let services = crate::service_resolution::directory(document)?
        .map(|directory| directory.services)
        .unwrap_or_default();
    for (product, policy) in &control.products {
        let location = format!("registry.release_control.products.{product}");
        if !identifier(product) || !services.contains_key(&policy.service) {
            return Err(format!("{location}: product or logical service is invalid"));
        }
        if policy.config_schema == 0
            || policy.state_schema == 0
            || !safe_install_root(&policy.install_root)
            || !safe_relative(&policy.binary)
            || !safe_relative(&policy.launcher)
            || !env_name(&policy.binary_env)
            || !env_name(&policy.port_env)
            || !env_name(&policy.runtime_env)
        {
            return Err(format!(
                "{location}: paths or environment variable names are invalid"
            ));
        }
        if policy.targets.is_empty() {
            return Err(format!("{location}.targets must not be empty"));
        }
        if policy.strategy.readiness_timeout_seconds == 0
            || policy.strategy.drain_timeout_seconds == 0
            || policy.strategy.rollback_window_seconds < policy.strategy.drain_timeout_seconds
        {
            return Err(format!("{location}.strategy has invalid time bounds"));
        }
        for (name, value) in &policy.environment {
            if !env_name(name) || value.is_empty() || value.chars().any(char::is_control) {
                return Err(format!("{location}.environment.{name} is invalid"));
            }
        }
        let mut platforms = BTreeSet::new();
        for (target, target_policy) in &policy.targets {
            if !target_names.contains(target.as_str()) {
                return Err(format!("{location}.targets.{target}: unknown target"));
            }
            // Blue-green and replace share the readiness contract, and only
            // blue-green owns serving coordinates for a second candidate.
            //
            // A replace target may OMIT `readiness_path` and take
            // [`DEFAULT_REPLACE_READINESS_PATH`]. That is not a convenience:
            // requiring the key here made this document unwritable by the
            // fleet that has to obey it. Stado 0.13.20 and 0.13.23 REFUSE a
            // replace target carrying `readiness_path` at all — "replace
            // rollout forbids stable_bind, candidate_ports and
            // readiness_path" — and this validator required it, so on
            // 2026-09-01 no single registry document satisfied both: with the
            // key present the always-on Mac's 0.13.20 queue agent could
            // resolve no policy and stopped scanning its disk for twelve
            // minutes; with it absent every write from the operator's own
            // installed 0.13.26 binary was refused. The fleet could only be
            // written by a build older than the one it was running, and the
            // workaround was to keep that older build around. Validation is
            // whole-document, so one field in one product's rollout froze
            // every domain — instance 16's blast radius with instance 17's
            // version skew.
            //
            // Accepting the absence is the additive shape: it admits both the
            // old constraint and the new one, and any document written for
            // either version validates under both for as long as both exist.
            let readiness_path = match policy.strategy.kind {
                StrategyKind::Replace => target_policy
                    .readiness_path
                    .as_deref()
                    .unwrap_or(DEFAULT_REPLACE_READINESS_PATH),
                StrategyKind::BlueGreen => {
                    target_policy.readiness_path.as_deref().ok_or_else(|| {
                        format!(
                            "{location}.targets.{target}: blue-green rollout requires readiness_path"
                        )
                    })?
                }
            };
            let serving = match policy.strategy.kind {
                StrategyKind::BlueGreen => Some(target_policy.blue_green_serving().map_err(
                    |_| {
                        format!(
                            "{location}.targets.{target}: blue-green rollout requires stable_bind, candidate_ports and readiness_path"
                        )
                    },
                )?),
                StrategyKind::Replace => {
                    if target_policy.stable_bind.is_some()
                        || target_policy.candidate_ports.is_some()
                    {
                        return Err(format!(
                            "{location}.targets.{target}: replace rollout forbids stable_bind and candidate_ports"
                        ));
                    }
                    None
                }
            };
            if !identifier(&target_policy.platform)
                || !identifier(&target_policy.run_as_user)
                || !safe_absolute(&target_policy.home)
                || !safe_absolute(&target_policy.state_dir)
                || !safe_absolute(&target_policy.runtime_root)
                || !safe_absolute(&target_policy.logs_root)
                || target_policy
                    .legacy_launchd_plist
                    .as_deref()
                    .is_some_and(|path| !safe_absolute(path))
                || !readiness_path.starts_with('/')
                || readiness_path.contains("..")
            {
                return Err(format!("{location}.targets.{target}: invalid platform, identity, path, or readiness path"));
            }
            match (
                target_policy.legacy_launchd_label.as_deref(),
                target_policy.legacy_launchd_plist.as_deref(),
            ) {
                (None, None) => {}
                (Some(label), Some(plist)) if valid_legacy_launchd_unit(label, plist) => {}
                (Some(_), Some(_)) => {
                    return Err(format!(
                        "{location}.targets.{target}: invalid legacy launchd label or path"
                    ));
                }
                _ => {
                    return Err(format!(
                        "{location}.targets.{target}: legacy launchd label and plist must be declared together"
                    ));
                }
            }
            if let Some(serving) = &serving {
                let bind: SocketAddr = serving.stable_bind.parse().map_err(|_| {
                    format!("{location}.targets.{target}.stable_bind is not a socket address")
                })?;
                if !bind.ip().is_loopback()
                    || serving.candidate_ports[0] == serving.candidate_ports[1]
                    || serving.candidate_ports.contains(&bind.port())
                {
                    return Err(format!(
                        "{location}.targets.{target}: blue-green ports are invalid"
                    ));
                }
            }
            platforms.insert(target_policy.platform.clone());
        }
        for desired in [policy.desired.as_ref(), policy.previous.as_ref()]
            .into_iter()
            .flatten()
        {
            if !canonical_coordinate(&desired.version) || desired.rollout_generation == 0 {
                return Err(format!(
                    "{location}.desired release coordinate or generation is invalid"
                ));
            }
            for platform in &platforms {
                let artifact = desired.artifacts.get(platform).ok_or_else(|| {
                    format!("{location}.desired.artifacts: missing platform {platform}")
                })?;
                for uri in [
                    &artifact.manifest_uri,
                    &artifact.signature_uri,
                    &artifact.archive_uri,
                ] {
                    if !uri.starts_with(&format!(
                        "stado://releases/{product}/{}/{platform}/",
                        desired.version
                    )) {
                        return Err(format!(
                            "{location}.desired artifact URI is outside its immutable coordinate"
                        ));
                    }
                }
                if !sha256(&artifact.manifest_sha256)
                    || !sha256(&artifact.artifact_sha256)
                    || !control.trusted_keys.contains_key(&artifact.key_id)
                {
                    return Err(format!(
                        "{location}.desired artifact trust metadata is invalid"
                    ));
                }
            }
        }
    }
    Ok(())
}
