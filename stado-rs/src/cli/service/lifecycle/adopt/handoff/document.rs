//! The two registry edits the handoff makes: externalizing the placement
//! profile's unit, and dropping the legacy launchd identity release control
//! would otherwise still reach for.

use super::*;

pub(super) fn externalize_release_controlled_profile(
    document: &mut Value,
    profile: &str,
    service_name: &str,
    product: &str,
) -> Result<(), CmdError> {
    let profiles = document
        .get_mut("placement_profiles")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| CmdError::click("registry.placement_profiles is not an array"))?;
    let profile = profiles
        .iter_mut()
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some(profile))
        .ok_or_else(|| CmdError::click(format!("placement profile {profile:?} disappeared")))?;
    let hosts = profile
        .get_mut("hosts")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| CmdError::click("placement profile hosts is not an object"))?;
    for (host, template) in hosts {
        let units = template
            .get_mut("units")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                CmdError::click(format!("placement host {host:?} units is not an object"))
            })?;
        if !units.contains_key(service_name) {
            return Err(CmdError::click(format!(
                "placement host {host:?} has no template for service {service_name:?}"
            )));
        }
        units.insert(
            service_name.to_string(),
            json!({
                "name": service_name,
                "controller": "release-control",
                "product": product,
            }),
        );
    }
    Ok(())
}

pub(super) fn remove_release_legacy_identity(
    document: &mut Value,
    product: &str,
    host: &str,
) -> Result<(), CmdError> {
    let target = document
        .get_mut("release_control")
        .and_then(|control| control.get_mut("products"))
        .and_then(|products| products.get_mut(product))
        .and_then(|policy| policy.get_mut("targets"))
        .and_then(|targets| targets.get_mut(host))
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            CmdError::click(format!(
                "release-control product {product:?} target {host:?} disappeared"
            ))
        })?;
    target.remove("legacy_launchd_label");
    target.remove("legacy_launchd_plist");
    let generation = document
        .get_mut("release_control")
        .and_then(Value::as_object_mut)
        .and_then(|control| control.get_mut("generation"))
        .and_then(|value| value.as_u64())
        .ok_or_else(|| CmdError::click("registry.release_control.generation is not an integer"))?;
    document["release_control"]["generation"] = Value::from(generation.saturating_add(1));
    Ok(())
}
