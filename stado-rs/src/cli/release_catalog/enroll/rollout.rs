//! The rollout policy of a product that declares a runtime: created by Stado
//! when the registry has none, so a new service does not wait for somebody to
//! write its `release_control.products` entry by hand.
//!
//! Every value comes from something the fleet already states. The host is the
//! one the service directory places the service on, or the vault owner. Its
//! user, home and platform come from that host's registry entry. Binary,
//! launcher and schemas come from the manifest's `runtime`; the stable port
//! from `runtime.port`. The two candidate ports are the next two no other
//! policy uses, and the rollout strategy is the one the fleet's existing
//! blue-green policies already run with.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use crate::cli::{registry, CmdError};
use crate::release_control::DEFAULT_REPLACE_READINESS_PATH;
use crate::release_pipeline::RuntimeContract;

use super::super::publisher::fleet_hosts;

/// Create `product`'s rollout policy when the registry has none.
pub(super) async fn ensure_rollout_policy(
    product: &str,
    runtime: &RuntimeContract,
) -> Result<Value, CmdError> {
    let (document, _) = registry::fetch_versioned_document().await?;
    if document
        .pointer(&format!("/release_control/products/{product}"))
        .is_some()
    {
        return Ok(json!({ "step": "rollout-policy", "product": product, "created": false }));
    }
    let Some(port) = runtime.port else {
        return Ok(json!({
            "step": "rollout-policy",
            "product": product,
            "created": false,
            "finding": "runtime declares no port, so no blue-green target can be created; add runtime.port to .wisent-release.json",
        }));
    };
    let host = match document
        .pointer(&format!(
            "/service_directory/services/{product}/active_host"
        ))
        .and_then(Value::as_str)
    {
        Some(placed) => placed.to_string(),
        None => fleet_hosts().await?.0,
    };
    let policy = policy_for(&document, product, runtime, &host, port)?;
    let written = policy.clone();
    let generation = registry::commit_document(move |current| {
        let mut next = current.clone();
        let products = next
            .pointer_mut("/release_control/products")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| CmdError::click("registry.release_control.products is not an object"))?;
        if products.contains_key(product) {
            return Ok(next);
        }
        products.insert(product.to_string(), written.clone());
        let generation = next
            .pointer("/release_control/generation")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                CmdError::click("registry.release_control.generation is not an integer")
            })?;
        next["release_control"]["generation"] = Value::from(generation.saturating_add(1));
        Ok(next)
    })
    .await?;
    eprintln!("{product}: rollout policy created with target {host} on 127.0.0.1:{port}");
    Ok(json!({
        "step": "rollout-policy",
        "product": product,
        "created": true,
        "target": host,
        "registry_generation": generation,
        "policy": policy,
    }))
}

/// The policy document, from the registry and the manifest's runtime.
fn policy_for(
    document: &Value,
    product: &str,
    runtime: &RuntimeContract,
    host: &str,
    port: u16,
) -> Result<Value, CmdError> {
    let target = document
        .get("targets")
        .and_then(Value::as_array)
        .and_then(|targets| {
            targets
                .iter()
                .find(|target| target.get("name").and_then(Value::as_str) == Some(host))
        })
        .ok_or_else(|| CmdError::click(format!("registry has no target {host:?}")))?;
    let platform = target
        .get("release_platform")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            CmdError::click(format!(
                "registry target {host} declares no release_platform"
            ))
        })?;
    let user = target
        .get("ssh")
        .and_then(Value::as_str)
        .and_then(|ssh| ssh.split_once('@'))
        .map(|(user, _)| user.to_string())
        .ok_or_else(|| CmdError::click(format!("registry target {host} declares no ssh user")))?;
    let home = if user == "root" {
        "/root".to_string()
    } else if platform.starts_with("darwin") {
        format!("/Users/{user}")
    } else {
        format!("/home/{user}")
    };
    let strategy = document
        .pointer("/release_control/products")
        .and_then(Value::as_object)
        .and_then(|products| {
            products.values().find_map(|policy| {
                policy
                    .get("strategy")
                    .filter(|strategy| strategy.get("kind").and_then(Value::as_str) == Some("blue-green"))
                    .cloned()
            })
        })
        .ok_or_else(|| {
            CmdError::click("no existing blue-green policy in release_control to take the rollout strategy from")
        })?;
    let (first, second) = free_candidate_ports(document, port)?;
    let prefix = product.to_uppercase().replace('-', "_");
    Ok(json!({
        "service": product,
        "config_schema": runtime.config_schema,
        "state_schema": runtime.state_schema,
        "install_root": format!("{{home}}/.stado/services/{product}"),
        "binary": runtime.binary,
        "launcher": runtime.launcher,
        "binary_env": format!("{prefix}_BIN"),
        "port_env": format!("{prefix}_PORT"),
        "runtime_env": format!("{prefix}_RUNTIME_DIR"),
        "environment": Map::new(),
        "signing_key_item": "",
        "signing_key_id": "",
        "strategy": strategy,
        "targets": {
            host: {
                "platform": platform,
                "run_as_user": user,
                "home": home,
                "state_dir": format!("{home}/.stado/release-state"),
                "runtime_root": format!("{home}/.stado/run"),
                "logs_root": format!("{home}/.stado/logs"),
                "stable_bind": format!("127.0.0.1:{port}"),
                "candidate_ports": [first, second],
                "readiness_path": runtime.readiness_path.clone().unwrap_or_else(|| DEFAULT_REPLACE_READINESS_PATH.to_string()),
                "legacy_launchd_label": null,
                "legacy_launchd_plist": null,
            }
        },
        "desired": null,
        "previous": null,
    }))
}

/// The two lowest ports above every port a release policy already binds.
fn free_candidate_ports(document: &Value, stable: u16) -> Result<(u16, u16), CmdError> {
    let mut used: BTreeSet<u16> = BTreeSet::from([stable]);
    let policies = document
        .pointer("/release_control/products")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|products| products.values());
    for policy in policies {
        let targets = policy
            .get("targets")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|targets| targets.values());
        for target in targets {
            used.extend(
                target
                    .get("candidate_ports")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_u64)
                    .filter_map(|port| u16::try_from(port).ok()),
            );
            used.extend(
                target
                    .get("stable_bind")
                    .and_then(Value::as_str)
                    .and_then(|bind| bind.rsplit_once(':'))
                    .and_then(|(_, port)| port.parse::<u16>().ok()),
            );
        }
    }
    let highest = used.iter().next_back().copied().unwrap_or(stable);
    let first = highest.checked_add(1);
    let second = first.and_then(|port| port.checked_add(1));
    match (first, second) {
        (Some(first), Some(second)) => Ok((first, second)),
        _ => Err(CmdError::click(
            "no free candidate ports above the ports release policies already use",
        )),
    }
}
