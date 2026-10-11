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
//!
//! A product whose catalog service names its one unit (`com.wisent.<product>`)
//! is rolled out by `replace` instead: a blue-green rollout runs the product
//! as release processes behind a proxy in the host's Stado, beside that unit,
//! which is the second process a product may not have. Its policy is created
//! with `replace`, and a blue-green policy it already has is converted; the
//! release agent then hands the port to the unit once the unit is loaded.

mod policy;
mod route;

use serde_json::{json, Value};

use crate::cli::{registry, CmdError};
use crate::release_pipeline::RuntimeContract;

use super::super::publisher::fleet_hosts;
use policy::policy_for;
use route::{declare_route, directory_route};

/// Create `product`'s rollout policy when the registry has none.
pub(super) async fn ensure_rollout_policy(
    product: &str,
    runtime: &RuntimeContract,
) -> Result<Value, CmdError> {
    let (document, _) = registry::fetch_versioned_document().await?;
    let one_unit = crate::deploy::service_catalog::lookup(product)
        .map_err(|error| {
            CmdError::click(error).stating(crate::primitives::failure::FailureCode::Config)
        })?
        .and_then(|entry| entry.unit);
    if let Some(existing) = document.pointer(&format!("/release_control/products/{product}")) {
        let blue_green =
            existing.pointer("/strategy/kind").and_then(Value::as_str) == Some("blue-green");
        return match one_unit {
            Some(unit) if blue_green => convert_to_replace(product, &unit).await,
            _ => Ok(json!({ "step": "rollout-policy", "product": product, "created": false })),
        };
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
    // A release-control policy must name a service the directory declares,
    // so a product the directory does not know yet gets its route in the
    // same write: on the policy's host, at its stable port, called by the
    // consumers the manifest names.
    let route = match document.pointer(&format!("/service_directory/services/{product}")) {
        Some(_) => None,
        None => Some(directory_route(product, runtime, &host, port)?),
    };
    let policy = policy_for(&document, product, runtime, &host, port, one_unit.is_some())?;
    let written = policy.clone();
    let written_route = route.clone();
    let route_host = host.clone();
    let generation = registry::commit_document(move |current| {
        let mut next = current.clone();
        let products = next
            .pointer_mut("/release_control/products")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                CmdError::click("registry.release_control.products is not an object")
                    .stating(crate::primitives::failure::FailureCode::Config)
            })?;
        if products.contains_key(product) {
            return Ok(next);
        }
        products.insert(product.to_string(), written.clone());
        let generation = next
            .pointer("/release_control/generation")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                CmdError::click("registry.release_control.generation is not an integer")
                    .stating(crate::primitives::failure::FailureCode::Config)
            })?;
        next["release_control"]["generation"] = Value::from(generation.saturating_add(1));
        if let Some(route) = &written_route {
            declare_route(&mut next, product, &route_host, route.clone())?;
        }
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
        "directory_route": route,
    }))
}

/// Turn `product`'s blue-green policy into a `replace` policy of its one unit
/// `unit`: the strategy kind, and every target without the proxy's stable
/// bind and candidate ports, which a replace target may not carry.
async fn convert_to_replace(product: &str, unit: &str) -> Result<Value, CmdError> {
    let generation = registry::commit_document(move |current| {
        let mut next = current.clone();
        let policy = next
            .pointer_mut(&format!("/release_control/products/{product}"))
            .ok_or_else(|| {
                CmdError::click(format!("{product} lost its rollout policy"))
                    .stating(crate::primitives::failure::FailureCode::NotFound)
            })?;
        if policy.pointer("/strategy/kind").and_then(Value::as_str) != Some("blue-green") {
            return Ok(next);
        }
        policy["strategy"]["kind"] = Value::from("replace");
        if let Some(targets) = policy.get_mut("targets").and_then(Value::as_object_mut) {
            for target in targets.values_mut() {
                target["stable_bind"] = Value::Null;
                target["candidate_ports"] = Value::Null;
            }
        }
        let generation = next
            .pointer("/release_control/generation")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                CmdError::click("registry.release_control.generation is not an integer")
                    .stating(crate::primitives::failure::FailureCode::Config)
            })?;
        next["release_control"]["generation"] = Value::from(generation.saturating_add(1));
        Ok(next)
    })
    .await?;
    eprintln!(
        "{product}: rollout policy converted from blue-green to replace of its one unit {unit}"
    );
    Ok(json!({
        "step": "rollout-policy",
        "product": product,
        "created": false,
        "converted": "blue-green -> replace",
        "unit": unit,
        "registry_generation": generation,
    }))
}
