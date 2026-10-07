//! The service directory route of a product enrolled before the directory
//! knew it. Every value comes from what the fleet already states: the host
//! the rollout policy targets, the manifest's stable `runtime.port`, and the
//! consumers the manifest's `runtime.consumers` names. It is the same entry
//! `stado service declare` writes for a fixed route: the active host, its
//! loopback endpoint (the directory contract requires host-relative
//! loopback), the managed service it links to, and a declared-only record of
//! that service on the host, which the first delivery replaces.

use std::net::Ipv4Addr;

use serde_json::{json, Map, Value};

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;
use crate::release_pipeline::RuntimeContract;

/// The route to declare for `product`, or the refusal naming what the
/// manifest is missing.
pub(super) fn directory_route(
    product: &str,
    runtime: &RuntimeContract,
    host: &str,
    port: u16,
) -> Result<Value, CmdError> {
    if runtime.consumers.is_empty() {
        return Err(CmdError::click(format!(
            "{product}: the service directory declares no route for {product}, and \
             .wisent-release.json names no runtime.consumers to declare one with; \
             add the consumer identities that call {product} to runtime.consumers"
        ))
        .stating(FailureCode::Config));
    }
    let consumers: Map<String, Value> = runtime
        .consumers
        .iter()
        .map(|consumer| (consumer.clone(), json!({})))
        .collect();
    Ok(json!({
        "active_host": host,
        "endpoints": { host: { "url": format!("http://{}:{port}", Ipv4Addr::LOCALHOST) } },
        "managed_service": product,
        "consumers": consumers,
    }))
}

/// Write `route` as `product`'s directory entry, record the service as
/// declared on `host`, and advance the directory's publication counter so
/// every consumer's cached copy learns the entry exists.
pub(super) fn declare_route(
    document: &mut Value,
    product: &str,
    host: &str,
    route: Value,
) -> Result<(), CmdError> {
    let services = document
        .pointer_mut("/service_directory/services")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            CmdError::click(
                "registry.service_directory.services is not an object; an authority must \
                 publish the service directory before a service can be enrolled",
            )
            .stating(FailureCode::Config)
        })?;
    if services.contains_key(product) {
        return Ok(());
    }
    services.insert(product.to_string(), route);
    let target = document
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .and_then(|targets| {
            targets
                .iter_mut()
                .find(|target| target.get("name").and_then(Value::as_str) == Some(host))
        })
        .ok_or_else(|| {
            CmdError::click(format!("registry has no target {host:?}"))
                .stating(FailureCode::NotFound)
        })?;
    let records = target
        .as_object_mut()
        .ok_or_else(|| {
            CmdError::click(format!("registry target {host} is not an object"))
                .stating(FailureCode::Config)
        })?
        .entry("services")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or_else(|| {
            CmdError::click(format!("registry target {host}: services must be an array"))
                .stating(FailureCode::Config)
        })?;
    if !records
        .iter()
        .any(|record| record.get("name").and_then(Value::as_str) == Some(product))
    {
        records.push(json!({ "name": product, "declared_only": true }));
    }
    crate::service_resolution::advance_generation(document).map_err(CmdError::declaration)
}
