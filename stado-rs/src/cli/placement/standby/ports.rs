//! Where a standby unit listens and where its probes ask, read from the
//! service directory.

use serde_json::{json, Value};

use crate::cli::CmdError;

/// The port a standby unit of `logical` listens on, from the service
/// directory: the address it names for `target` (as a standby, or as an
/// endpoint), else the address of the host the profile is placed on, which a
/// move keeps. `None` when the directory routes the service nowhere; a
/// directory address that carries no port is refused by name.
pub(super) fn standby_port(
    document: &Value,
    logical: &str,
    target: &str,
    placed_on: &str,
) -> Result<Option<u16>, CmdError> {
    let Some(route) = document
        .get("service_directory")
        .and_then(|directory| directory.get("services"))
        .and_then(|services| services.get(logical))
    else {
        return Ok(None);
    };
    let address_of = |map: &str, host: &str| route.get(map).and_then(|hosts| hosts.get(host));
    let address = address_of("standby", target)
        .or_else(|| address_of("endpoints", target))
        .or_else(|| address_of("endpoints", placed_on));
    let Some(address) = address else {
        return Ok(None);
    };
    let url = address.get("url").and_then(Value::as_str).ok_or_else(|| {
        CmdError::declaration(format!(
            "directory service {logical} names an address for {target} or {placed_on} without a \
             url"
        ))
    })?;
    let parsed = url::Url::parse(url).map_err(|error| {
        CmdError::declaration(format!("directory service {logical} url {url}: {error}"))
    })?;
    parsed.port().map(Some).ok_or_else(|| {
        CmdError::declaration(format!(
            "directory service {logical} url {url} names no port, so the standby unit has none \
             to listen on"
        ))
    })
}

/// The placed host's probes as they apply on `target`. Each probe of a
/// service the directory routes keeps its scheme, host and path and takes
/// the port the standby unit listens on there ([`standby_port`]): a host the
/// directory already names may serve on another port than the placed host,
/// and a probe copied verbatim asked that other port, so the move's health
/// check judged a port nothing on `target` listens on. A probe of a service
/// the directory does not route is kept as it is.
pub(super) fn probes_for(
    document: &Value,
    probes: &Value,
    target: &str,
    placed_on: &str,
) -> Result<Value, CmdError> {
    let listed = probes.as_array().ok_or_else(|| {
        CmdError::declaration(format!(
            "the probes declared for {placed_on} are not a list"
        ))
    })?;
    let mut moved = Vec::with_capacity(listed.len());
    for probe in listed {
        let service = probe
            .get("service")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CmdError::declaration(format!("a probe declared for {placed_on} names no service"))
            })?;
        let url = probe.get("url").and_then(Value::as_str).ok_or_else(|| {
            CmdError::declaration(format!(
                "the probe of {service} declared for {placed_on} names no url"
            ))
        })?;
        let mut next = probe.clone();
        if let Some(port) = standby_port(document, service, target, placed_on)? {
            let mut parsed = url::Url::parse(url).map_err(|error| {
                CmdError::declaration(format!("the probe of {service} url {url}: {error}"))
            })?;
            parsed.set_port(Some(port)).map_err(|()| {
                CmdError::declaration(format!(
                    "the probe of {service} url {url} cannot carry a port, so it cannot ask the \
                     standby unit on {target}"
                ))
            })?;
            next["url"] = json!(parsed.as_str());
        }
        moved.push(next);
    }
    Ok(Value::Array(moved))
}
