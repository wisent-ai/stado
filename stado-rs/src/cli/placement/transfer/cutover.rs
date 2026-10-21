//! The address each profile service has on the destination after the move.

use serde_json::{Map, Value};

use crate::cli::CmdError;

/// Give every directory service of `profile` an endpoint on `destination`.
///
/// `endpoints[<host>]` is the address a host calls and `standby[<host>]` the
/// address it would serve on after a move, which is what `placement standby`
/// writes. A cutover makes the destination the active host, so its standby
/// address becomes the address it calls: it is moved from `standby` into
/// `endpoints` in the same document the move commits. A destination the
/// directory names under neither map is refused naming the service, because
/// the cutover would otherwise move the service to a host nothing can reach.
pub(super) fn promote_standby(
    document: &mut Value,
    profile: &str,
    destination: &str,
) -> Result<(), CmdError> {
    let Some(services) = document
        .get_mut("service_directory")
        .and_then(|directory| directory.get_mut("services"))
    else {
        return Ok(());
    };
    let services = services.as_object_mut().ok_or_else(|| {
        CmdError::declaration("registry.service_directory.services is not an object")
    })?;
    for (name, route) in services.iter_mut() {
        let route = route.as_object_mut().ok_or_else(|| {
            CmdError::declaration(format!("directory service {name} is not an object"))
        })?;
        if route.get("placement_profile").and_then(Value::as_str) != Some(profile) {
            continue;
        }
        let named = |route: &Map<String, Value>, map: &str| {
            route
                .get(map)
                .and_then(|hosts| hosts.get(destination))
                .is_some()
        };
        if named(route, "endpoints") {
            continue;
        }
        if !named(route, "standby") {
            return Err(CmdError::declaration(format!(
                "directory service {name} names {destination} neither as an endpoint nor as a \
                 standby, so after the move no host could reach it there; run stado placement \
                 standby {profile} --host {destination} first"
            )));
        }
        let standby = route
            .get_mut("standby")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                CmdError::declaration(format!("directory service {name} standby is not an object"))
            })?;
        let address = standby.remove(destination).ok_or_else(|| {
            CmdError::declaration(format!(
                "directory service {name} standby lost {destination} while it was read"
            ))
        })?;
        if standby.is_empty() {
            route.remove("standby");
        }
        route
            .get_mut("endpoints")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                CmdError::declaration(format!(
                    "directory service {name} endpoints is not an object"
                ))
            })?
            .insert(destination.to_string(), address);
    }
    Ok(())
}
