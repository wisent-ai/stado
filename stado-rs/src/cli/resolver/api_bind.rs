//! `stado resolver api-reassign --target HOST`: give a host's resolution
//! API a loopback port that host hands out now (`stado host free-port-local`
//! run there), recorded as `targets.<host>.service_resolver.api_bind` under
//! the registry generation it was read at.
//!
//! The API's port used to be one somebody wrote into the registry, which is
//! how two products came to claim one port. The host's resolver reads the new
//! bind from the registry and restarts its role to rebind it; consumers ask
//! the resolver at every start, so none holds a copy of the old address.

use std::net::{Ipv4Addr, SocketAddrV4};

use serde_json::{json, Value};

use crate::cli::{registry, CmdError};

pub async fn api_reassign(target: &str, json_output: bool) -> Result<(), CmdError> {
    let registry_document = crate::targets::fetch_registry_remote()
        .await
        .map_err(CmdError::from)?;
    let resolved = crate::cli::resolved_host(&registry_document, target)?.clone();
    let port =
        crate::cli::directory::host_free_port(&resolved, &crate::deploy::production_runner())
            .await?;
    let bind = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).to_string();
    // The bind the resolver held before, for the receipt; a resolver that
    // declared none is told so rather than given an invented previous value.
    let mut previous: Option<Value> = None;
    let generation = registry::commit_document(|current| {
        let mut document = current.clone();
        let resolver = document
            .get_mut("targets")
            .and_then(Value::as_array_mut)
            .and_then(|targets| {
                targets
                    .iter_mut()
                    .find(|entry| entry.get("name").and_then(Value::as_str) == Some(target))
            })
            .and_then(|entry| entry.get_mut("service_resolver"))
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                CmdError::click(format!(
                    "{target} declares no service_resolver, so it has no resolution API to move; \
                     `stado registry pull --path targets.{target}` shows what it declares"
                ))
                .stating(crate::primitives::failure::FailureCode::NotFound)
            })?;
        previous = resolver.get("api_bind").cloned();
        resolver.insert("api_bind".to_string(), json!(bind));
        Ok(document)
    })
    .await?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": target, "previous": previous, "api_bind": bind, "generation": generation,
            }))?
        );
    } else {
        let before = match previous.as_ref().and_then(Value::as_str) {
            Some(bind) => format!("from {bind}"),
            None => "(none was declared)".to_string(),
        };
        println!(
            "{target}: resolver API {before} to {bind} (generation {generation}); the host's \
             resolver rebinds it when it reads the registry"
        );
    }
    Ok(())
}
