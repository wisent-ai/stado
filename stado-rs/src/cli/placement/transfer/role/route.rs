//! The directory route of a role move: which routes it may move, the
//! address the destination serves on, and the registry change that moves
//! the route.

use serde_json::{json, Value};

use crate::cli::CmdError;

/// The route of `service` checked for a role move from its active host to
/// `to_host`; the source host's name.
pub(super) fn checked_route(
    document: &Value,
    service: &str,
    to_host: &str,
) -> Result<String, CmdError> {
    let route = document
        .pointer(&format!("/service_directory/services/{service}"))
        .and_then(Value::as_object)
        .ok_or_else(|| CmdError::missing(format!("the service directory declares no {service}")))?;
    let managed = route
        .get("managed_service")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            CmdError::refused(format!(
            "directory service {service} names no managed_service; a placement profile moves it \
             with stado placement move"
        ))
        })?;
    if !crate::deploy::service_catalog::is_host_unit(managed).map_err(CmdError::declaration)? {
        return Err(CmdError::refused(format!(
            "directory service {service} is served by {managed}, not by the host Stado process; \
             give it a placement profile and move it with stado placement move"
        )));
    }
    let source = route
        .get("active_host")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            CmdError::declaration(format!("directory service {service} names no active_host"))
        })?;
    if source == to_host {
        return Err(CmdError::refused(format!(
            "{service} is already served by {to_host}"
        )));
    }
    Ok(source.to_string())
}

/// The address every host's Stado serves its object API on, from the
/// catalog's host process (`--bind`, `--port`): after the move it is the
/// active host's endpoint, where every caller's tunnel ends.
pub(super) fn served_address() -> Result<String, CmdError> {
    let host = crate::deploy::service_catalog::host_process().map_err(CmdError::declaration)?;
    let option = |name: &str| {
        let mut arguments = host.args.iter();
        arguments
            .find(|argument| *argument == name)
            .and_then(|_option| arguments.next())
            .cloned()
            .ok_or_else(|| {
                CmdError::declaration(format!(
                    "the catalog's host Stado process declares no {name}"
                ))
            })
    };
    Ok(format!(
        "http://{}:{}",
        option("--bind")?,
        option("--port")?
    ))
}

/// `current` with `to_host` as `service`'s active host under the next
/// generation: its endpoint is the address its own Stado serves on, its
/// standby address goes, and the source's endpoint goes too, since the
/// source's Stado now serves only the fenced copy there; callers on the
/// source resolve the service like any other host's. Refused when the route
/// no longer stands on `source`.
pub(super) fn switched(
    current: &Value,
    service: &str,
    source: &str,
    to_host: &str,
    served: &str,
) -> Result<Value, CmdError> {
    let mut next = current.clone();
    let route = next
        .pointer_mut(&format!("/service_directory/services/{service}"))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| CmdError::missing(format!("the service directory declares no {service}")))?;
    if route.get("active_host").and_then(Value::as_str) != Some(source) {
        return Err(CmdError::refused(format!(
            "directory service {service} no longer stands on {source}; the registry changed during \
             the move"
        )));
    }
    let standby_left = match route.get_mut("standby").and_then(Value::as_object_mut) {
        Some(standby) => {
            standby.remove(to_host);
            Some(standby.is_empty())
        }
        None => None,
    };
    if standby_left == Some(true) {
        route.remove("standby");
    }
    let endpoints = route
        .entry("endpoints")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| CmdError::declaration(format!("{service} endpoints is not an object")))?;
    endpoints.remove(source);
    endpoints.insert(to_host.to_string(), json!({ "url": served }));
    route.insert("active_host".to_string(), json!(to_host));
    crate::service_resolution::advance_generation(&mut next).map_err(CmdError::declaration)?;
    Ok(next)
}
