//! `stado placement profile draft`: the placement profile that would move a
//! set of fixed directory routes, read from the registry, so a service that
//! was declared as a fixed route can be given a profile without writing its
//! unit file path and kind by hand. The inverse of `profile remove`, which
//! turns a profile's services back into fixed routes.

use serde_json::{json, Map, Value};

use crate::cli::CmdError;
use crate::deploy::service::ManagedService;

/// What the operator states that the registry cannot: where a service
/// answers its health probe when the catalog declares no GET answering
/// request for its product, and which stores travel with it.
pub(super) struct DraftInputs<'a> {
    pub name: &'a str,
    pub services: &'a [String],
    /// `SERVICE=URL`, at most one per service.
    pub probes: &'a [String],
    pub state: &'a [String],
    pub tree_state: &'a [String],
}

fn route<'a>(document: &'a Value, service: &str) -> Result<&'a Map<String, Value>, CmdError> {
    document
        .get("service_directory")
        .and_then(|directory| directory.get("services"))
        .and_then(|services| services.get(service))
        .and_then(Value::as_object)
        .ok_or_else(|| {
            CmdError::missing(format!("the service directory declares no service {service}"))
        })
}

/// The unit `managed` names among the services `host` declares, in the shape
/// a placement profile holds it.
fn declared_unit(document: &Value, host: &str, managed: &str) -> Result<Value, CmdError> {
    let target = document
        .get("targets")
        .and_then(Value::as_array)
        .and_then(|targets| {
            targets
                .iter()
                .find(|target| target.get("name").and_then(Value::as_str) == Some(host))
        })
        .ok_or_else(|| CmdError::missing(format!("{host} is not a registry target")))?;
    let records = match target.get("services") {
        Some(Value::Array(records)) => records,
        Some(_) => {
            return Err(CmdError::declaration(format!(
                "registry target {host} has a services key that is not an array"
            )))
        }
        None => {
            return Err(CmdError::declaration(format!(
                "registry target {host} declares no services, so it runs no {managed}"
            )))
        }
    };
    let service = records
        .iter()
        .filter_map(Value::as_object)
        .map(|record| ManagedService::from_record(host, record))
        .find(|service| service.matches(managed))
        .ok_or_else(|| {
            CmdError::declaration(format!(
                "registry target {host} declares no managed service {managed}"
            ))
        })?;
    // The file lives under the host's own home, which this machine cannot
    // know, so a record that never named it is refused rather than guessed.
    if service.path.is_empty() {
        return Err(CmdError::declaration(format!(
            "registry target {host} declares managed service {managed} with no unit file path, \
             which a placement profile must name; record the unit with stado service ensure \
             --host {host} first"
        )));
    }
    Ok(json!({
        "kind": service.kind,
        "name": managed,
        "path": service.path,
        "unit": service.unit_id(),
    }))
}

/// The probe the catalog declares for the product `managed` runs — its
/// `answering` request, when that is a GET with no body — asked at the
/// address the directory names for `service` on `host`. `None` when the
/// catalog declares no such request or the directory names no address there.
fn catalog_probe(
    document: &Value,
    host: &str,
    service: &str,
    managed: &str,
) -> Result<Option<String>, CmdError> {
    let Some(entry) =
        crate::deploy::service_catalog::lookup(managed).map_err(CmdError::declaration)?
    else {
        return Ok(None);
    };
    let Some(answering) = entry
        .answering
        .filter(|probe| probe.method == reqwest::Method::GET.as_str() && probe.body.is_none())
    else {
        return Ok(None);
    };
    let Some(address) = route(document, service)?
        .get("endpoints")
        .and_then(|endpoints| endpoints.get(host))
        .and_then(|address| address.get("url"))
        .and_then(Value::as_str)
    else {
        return Ok(None);
    };
    let base = url::Url::parse(address).map_err(|error| {
        CmdError::declaration(format!("directory service {service} url {address} on {host}: {error}"))
    })?;
    let url = base.join(&answering.path).map_err(|error| {
        CmdError::declaration(format!(
            "the catalog's answering path {} of {managed}: {error}",
            answering.path
        ))
    })?;
    Ok(Some(String::from(url)))
}

/// The probe of every service on `host`: from a `SERVICE=URL` argument, else
/// the catalog's answering request at the service's directory address there
/// ([`catalog_probe`]). A service named twice, a name that is not one of the
/// services, and a service neither gives a probe are refused naming it. The
/// URL is judged by the registry when the profile is declared.
fn probes(
    document: &Value,
    host: &str,
    managed_units: &[(String, String)],
    inputs: &DraftInputs<'_>,
) -> Result<Vec<Value>, CmdError> {
    let mut probes = Vec::new();
    let mut probed: Vec<&str> = Vec::new();
    for argument in inputs.probes {
        let Some((service, url)) = argument.split_once('=') else {
            return Err(CmdError::click(format!(
                "--probe {argument} is not SERVICE=URL"
            )));
        };
        if !inputs.services.iter().any(|named| named == service) {
            return Err(CmdError::click(format!(
                "--probe {argument} names {service}, which is not one of the drafted services"
            )));
        }
        if probed.contains(&service) {
            return Err(CmdError::click(format!("--probe names {service} twice")));
        }
        probed.push(service);
        probes.push(json!({ "service": service, "url": url }));
    }
    let mut unprobed: Vec<&str> = Vec::new();
    for (service, managed) in managed_units {
        if probed.contains(&service.as_str()) {
            continue;
        }
        match catalog_probe(document, host, service, managed)? {
            Some(url) => probes.push(json!({ "service": service, "url": url })),
            None => unprobed.push(service),
        }
    }
    if !unprobed.is_empty() {
        return Err(CmdError::click(format!(
            "a placement profile probes every service on its host, and neither a --probe \
             SERVICE=URL nor a GET answering request the catalog declares for its product names \
             {}",
            unprobed.join(", ")
        )));
    }
    Ok(probes)
}

/// The stores the catalog declares for the products the drafted units run,
/// in the order the services are named, each path once.
fn catalog_state(
    managed_units: &[(String, String)],
) -> Result<Vec<crate::deploy::service_catalog::CatalogState>, CmdError> {
    let mut stores: Vec<crate::deploy::service_catalog::CatalogState> = Vec::new();
    for (service, managed) in managed_units {
        let Some(entry) = super::stores::catalog_product(&[service.as_str(), managed.as_str()])?
        else {
            continue;
        };
        for store in entry.state {
            if !stores.iter().any(|kept| kept.path == store.path) {
                stores.push(store);
            }
        }
    }
    Ok(stores)
}

/// The profile named `inputs.name` holding `inputs.services`, each a fixed
/// route today, placed on the host they all run on. A service another
/// profile already moves, a route with no `managed_service` or
/// `active_host`, and services active on different hosts are refused, since
/// one profile moves from one host.
pub(super) fn draft(document: &Value, inputs: &DraftInputs<'_>) -> Result<Value, CmdError> {
    if inputs.services.is_empty() {
        return Err(CmdError::click("name at least one --service to draft"));
    }
    let mut host: Option<&str> = None;
    let mut units = Map::new();
    let mut managed_units: Vec<(String, String)> = Vec::new();
    for service in inputs.services {
        let route = route(document, service)?;
        if let Some(profile) = route.get("placement_profile").and_then(Value::as_str) {
            return Err(CmdError::refused(format!(
                "directory service {service} already moves with placement profile {profile}"
            )));
        }
        let active = route.get("active_host").and_then(Value::as_str).ok_or_else(|| {
            CmdError::declaration(format!("directory service {service} names no active_host"))
        })?;
        let managed = route.get("managed_service").and_then(Value::as_str).ok_or_else(|| {
            CmdError::declaration(format!(
                "directory service {service} names no managed_service, so no unit can be moved"
            ))
        })?;
        // The host Stado process runs on every host, so a move would find it
        // already running on the destination and fence the whole process on
        // the source; a role it serves is not a unit a profile can move.
        let host_process = crate::deploy::service_catalog::is_host_unit(managed)
            .map_err(CmdError::declaration)?;
        if host_process {
            return Err(CmdError::refused(format!(
                "directory service {service} is served by {managed}, the host Stado process \
                 that runs on every host; a placement profile fences and starts units, so it \
                 cannot carry a role of that process: move it with stado placement role move \
                 {service} --to-host <host> --reason <why>"
            )));
        }
        match host {
            Some(placed) if placed != active => {
                return Err(CmdError::refused(format!(
                    "directory service {service} is active on {active} and the others on \
                     {placed}; one placement profile moves from one host"
                )))
            }
            Some(_) => {}
            None => host = Some(active),
        }
        units.insert(service.clone(), declared_unit(document, active, managed)?);
        managed_units.push((service.clone(), managed.to_string()));
    }
    let Some(host) = host else {
        return Err(CmdError::click("name at least one --service to draft"));
    };
    let mut state: Vec<Value> = inputs
        .state
        .iter()
        .map(|path| json!({ "path": path, "required": true }))
        .chain(
            inputs
                .tree_state
                .iter()
                .map(|path| json!({ "path": path, "required": true, "tree": true })),
        )
        .collect();
    // The stores the catalog declares for the drafted products travel too;
    // a path the operator named keeps the operator's entry.
    for store in catalog_state(&managed_units)? {
        if inputs.state.contains(&store.path) || inputs.tree_state.contains(&store.path) {
            continue;
        }
        state.push(json!({ "path": store.path, "required": store.required, "tree": store.tree }));
    }
    let profile = json!({
        "name": inputs.name,
        "services": inputs.services,
        "stop_order": inputs.services.iter().rev().collect::<Vec<_>>(),
        "start_order": inputs.services,
        "state": state,
        "hosts": { host: { "units": units, "probes": probes(document, host, &managed_units, inputs)? } },
    });
    super::document::parse(&profile)?;
    Ok(profile)
}
