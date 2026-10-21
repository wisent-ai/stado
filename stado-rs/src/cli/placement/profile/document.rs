//! The registry transforms behind `stado placement profile`: one profile and
//! the service directory's links to it, written in one document so a reader
//! never sees a profile its routes do not name, or a route naming a profile
//! the document does not hold.

use serde_json::{Map, Value};

use crate::cli::CmdError;
use crate::placement::{self, PlacementProfile};

const PROFILES_KEY: &str = "placement_profiles";

/// The profile `candidate` declares, read with the registry's own model, so a
/// field the registry does not know is refused here rather than at the push.
pub(super) fn parse(candidate: &Value) -> Result<PlacementProfile, CmdError> {
    serde_json::from_value(candidate.clone()).map_err(|error| {
        CmdError::declaration(format!(
            "the placement profile does not parse as the registry reads one: {error}"
        ))
    })
}

fn declared(document: &Value) -> Result<Vec<PlacementProfile>, CmdError> {
    placement::profiles(document).map_err(CmdError::declaration)
}

fn refuse_while_moving(document: &Value, name: &str) -> Result<(), CmdError> {
    let transactions = placement::transactions(document).map_err(CmdError::declaration)?;
    match transactions.iter().find(|held| held.profile == name) {
        Some(held) => Err(CmdError::refused(format!(
            "placement profile {name} is held by the placement transaction {} moving it from \
             {} to {}; change it once that move has committed or rolled back",
            held.id, held.from_host, held.to_host
        ))),
        None => Ok(()),
    }
}

fn not_an_object(what: &str) -> CmdError {
    CmdError::declaration(format!("{what} is not an object"))
}

fn directory_services(document: &mut Value) -> Result<&mut Map<String, Value>, CmdError> {
    let services = document
        .get_mut("service_directory")
        .and_then(|directory| directory.get_mut("services"))
        .ok_or_else(|| {
            CmdError::declaration(
                "the registry holds no service_directory.services, so no service can be linked \
                 to a placement profile",
            )
        })?;
    services
        .as_object_mut()
        .ok_or_else(|| not_an_object("registry.service_directory.services"))
}

fn route_of<'a>(
    services: &'a mut Map<String, Value>,
    service: &str,
) -> Result<&'a mut Map<String, Value>, CmdError> {
    let route = services.get_mut(service).ok_or_else(|| {
        CmdError::missing(format!(
            "service {service} is not in the service directory; declare it with stado service \
             declare before placing it in a profile"
        ))
    })?;
    route
        .as_object_mut()
        .ok_or_else(|| not_an_object(&format!("directory service {service}")))
}

fn profiles_of(document: &mut Value) -> Result<&mut Vec<Value>, CmdError> {
    let root = placement::root_object(document).map_err(CmdError::declaration)?;
    root.entry(PROFILES_KEY)
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| CmdError::declaration(format!("registry.{PROFILES_KEY} is not an array")))
}

/// The document with `candidate` declared: added, or replacing the profile
/// of the same name, and every service it lists linked to it in the
/// directory. A service another profile holds is refused, and so is
/// dropping a service from a profile that already holds it: its route would
/// keep naming a profile that no longer moves it.
pub(super) fn declare(current: &Value, candidate: &Value) -> Result<Value, CmdError> {
    let profile = parse(candidate)?;
    refuse_while_moving(current, &profile.name)?;
    let existing = declared(current)?;
    for other in existing.iter().filter(|other| other.name != profile.name) {
        let shared = other
            .services
            .iter()
            .find(|service| profile.services.contains(service));
        if let Some(service) = shared {
            return Err(CmdError::refused(format!(
                "service {service} already moves with placement profile {}; a service belongs \
                 to one profile, so remove it there first",
                other.name
            )));
        }
    }
    if let Some(previous) = existing.iter().find(|other| other.name == profile.name) {
        let dropped = previous
            .services
            .iter()
            .find(|service| !profile.services.contains(service));
        if let Some(service) = dropped {
            return Err(CmdError::refused(format!(
                "placement profile {} holds service {service}, which this declaration drops; \
                 remove the profile with stado placement profile remove and declare it again",
                profile.name
            )));
        }
    }
    let mut next = current.clone();
    let profiles = profiles_of(&mut next)?;
    let same_name = |entry: &&mut Value| {
        entry.get("name").and_then(Value::as_str) == Some(profile.name.as_str())
    };
    match profiles.iter_mut().find(same_name) {
        Some(entry) => *entry = candidate.clone(),
        None => profiles.push(candidate.clone()),
    }
    let services = directory_services(&mut next)?;
    for service in &profile.services {
        let route = route_of(services, service)?;
        let link = Value::String(profile.name.clone());
        route.insert("placement_profile".to_string(), link);
        // A placement-backed route derives its managed unit from the profile.
        route.remove("managed_service");
    }
    crate::service_resolution::advance_generation(&mut next).map_err(CmdError::declaration)?;
    Ok(next)
}

/// The document without the profile `name`. Each of its services becomes a
/// fixed route again, naming the unit the profile declares on the route's
/// active host, and loses the standby addresses only a profile gives meaning.
pub(super) fn remove(current: &Value, name: &str) -> Result<Value, CmdError> {
    let profile = declared(current)?
        .into_iter()
        .find(|profile| profile.name == name)
        .ok_or_else(|| {
            CmdError::missing(format!(
                "the registry declares no placement profile named {name}"
            ))
        })?;
    refuse_while_moving(current, name)?;
    let mut next = current.clone();
    let services = directory_services(&mut next)?;
    for service in &profile.services {
        let route = route_of(services, service)?;
        let active = match route.get("active_host").and_then(Value::as_str) {
            Some(active) => active.to_string(),
            None => {
                return Err(CmdError::declaration(format!(
                    "directory service {service} names no active_host"
                )))
            }
        };
        let unit = profile
            .hosts
            .get(&active)
            .and_then(|host| host.units.get(service))
            .ok_or_else(|| {
                CmdError::declaration(format!(
                    "placement profile {name} declares no unit for {service} on its active host \
                     {active}, so the fixed route it leaves behind could name no managed service"
                ))
            })?;
        route.remove("placement_profile");
        route.remove("standby");
        let managed = Value::String(unit.name.clone());
        route.insert("managed_service".to_string(), managed);
    }
    let profiles = profiles_of(&mut next)?;
    profiles.retain(|entry| entry.get("name").and_then(Value::as_str) != Some(name));
    crate::service_resolution::advance_generation(&mut next).map_err(CmdError::declaration)?;
    Ok(next)
}
