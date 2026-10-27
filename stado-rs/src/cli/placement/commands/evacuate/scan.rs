//! What an evacuation finds on the host it empties: the profiles placed
//! there, the fixed routes and the roles of the host Stado process active
//! there.

use serde_json::Value;

use super::super::moves::placed_host;
use crate::cli::placement::candidates::parse_registry;
use crate::cli::CmdError;
use crate::placement::{self, PlacementProfile};

/// What the registry says about `host`: the profiles placed on it, in the
/// registry's order, and each profile whose placement cannot be told, with
/// why (it may stand on `host` beside another).
pub(super) struct Placed {
    pub profiles: Vec<PlacementProfile>,
    pub untold: Vec<String>,
}

pub(super) fn placed_on(document: &Value, host: &str) -> Result<Placed, CmdError> {
    let registry = parse_registry(document)?;
    let mut placed = Placed {
        profiles: Vec::new(),
        untold: Vec::new(),
    };
    for profile in placement::profiles(document).map_err(CmdError::declaration)? {
        match placed_host(&registry, &profile) {
            Ok(source) if source == host => placed.profiles.push(profile),
            Ok(_) => {}
            Err(cause) => placed.untold.push(format!("{}: {cause}", profile.name)),
        }
    }
    Ok(placed)
}

/// The directory services active on a host that no profile moves: the
/// fixed routes, and the roles of the host Stado process, which a role move
/// carries.
pub(super) struct Fixed {
    pub routes: Vec<String>,
    pub roles: Vec<String>,
}

pub(super) fn fixed_routes_on(document: &Value, host: &str) -> Result<Fixed, CmdError> {
    let mut fixed = Fixed {
        routes: Vec::new(),
        roles: Vec::new(),
    };
    let Some(services) = document
        .get("service_directory")
        .and_then(|directory| directory.get("services"))
        .and_then(Value::as_object)
    else {
        return Ok(fixed);
    };
    for (name, route) in services {
        if route.get("active_host").and_then(Value::as_str) != Some(host)
            || route.get("placement_profile").is_some()
        {
            continue;
        }
        let role = match route.get("managed_service").and_then(Value::as_str) {
            Some(managed) => crate::deploy::service_catalog::is_host_unit(managed)
                .map_err(CmdError::declaration)?,
            None => false,
        };
        if role {
            fixed.roles.push(name.clone());
        } else {
            fixed.routes.push(name.clone());
        }
    }
    Ok(fixed)
}

/// The release-control products that roll out to `host`: each keeps
/// delivering there after the host's services leave, until `stado release
/// policy remove-target` drops the host.
pub(super) fn rolled_out_on(document: &Value, host: &str) -> Vec<String> {
    document
        .get("release_control")
        .and_then(|control| control.get("products"))
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(_, product)| product.get("targets").and_then(|targets| targets.get(host)).is_some())
        .map(|(name, _)| name.clone())
        .collect()
}

/// The release-control products an evacuation asked to drop `from` would
/// stop releasing there. A product whose only target is `from` cannot lose
/// it (a rollout policy rolls out somewhere), so it is a refusal naming
/// `stado release policy remove`, read before anything moves — unless the
/// run stands B by for a profile running that product's service, since
/// standby declares B a target of it before the drop. `drafted` is the
/// fixed routes given a profile in a run that stands B by: a check only
/// drafts them, so their services are counted here as the run will place
/// them.
pub(super) fn release_drops(
    document: &Value,
    from: &str,
    asked: bool,
    stood_by: &[&PlacementProfile],
    drafted: Option<&super::fixed::Profiled>,
    refusals: &mut Vec<String>,
) -> Vec<String> {
    if !asked {
        return Vec::new();
    }
    let mut services: Vec<&str> = stood_by.iter().flat_map(|profile| profile.services.iter()).map(String::as_str).collect();
    services.extend(drafted.into_iter().flat_map(|profiled| profiled.drafted.iter()).filter_map(|drafted| drafted["service"].as_str()));
    let products = rolled_out_on(document, from);
    for product in &products {
        let policy = &document["release_control"]["products"][product.as_str()];
        let elsewhere = policy["targets"].as_object().into_iter().flatten().any(|(host, _)| host != from);
        let service = policy["service"].as_str();
        let gains_target = services.iter().any(|run| *run == product.as_str() || Some(*run) == service);
        if !elsewhere && !gains_target {
            refusals.push(format!(
                "--drop-release-targets: {from} is the only target of {product}; stop releasing it by release control with stado release policy remove {product}"
            ));
        }
    }
    products
}

/// The host the service directory names the registry authority, or `None`
/// when the directory names none.
pub(super) fn authority_holder(document: &Value) -> Option<&str> {
    document["service_directory"]["authority"]["target"].as_str()
}

/// Each release-controlled product whose install root sends `to` outside its
/// own home (written under another target's home, `release_control::
/// foreign_targets`): after the evacuation `to` is where the product runs,
/// and its agent would install it in a directory that is not its own. With
/// `rewrite` (`--portable-roots`) they are returned for the run to write under
/// `{home}` first; without it each is a refusal. A product of a profile the
/// run stands `to` by is left out: the standby writes that root under `{home}`
/// itself. A release-control block that does not parse is a refusal too.
pub(super) fn foreign_roots_for(document: &Value, to: &str, stood_by: &[&PlacementProfile], rewrite: bool, refusals: &mut Vec<String>) -> Vec<String> {
    let block = &document[crate::release_control::RELEASE_CONTROL_KEY];
    if block.is_null() {
        return Vec::new();
    }
    let control: crate::release_control::ReleaseControl = match serde_json::from_value(block.clone()) {
        Ok(control) => control,
        Err(error) => {
            refusals.push(format!("the registry's release_control does not parse, so where {to} installs releases cannot be read: {error}"));
            return Vec::new();
        }
    };
    let rewritten = |product: &str, service: &str| {
        stood_by.iter().flat_map(|profile| profile.services.iter()).any(|run| run == product || run == service)
    };
    let mut portable = Vec::new();
    for (product, policy) in control.products.iter().filter(|(product, policy)| !rewritten(product.as_str(), policy.service.as_str())) {
        for foreign in crate::release_control::foreign_targets(policy).into_iter().filter(|foreign| foreign.host == to) {
            if rewrite {
                portable.push(product.clone());
                continue;
            }
            refusals.push(format!(
                "release-controlled product {product} installs every target at {}, under {}'s home, so {to} (home {}) \
                 would install outside its home; write it as {} with stado release policy portable-root {product}, or \
                 give --portable-roots",
                policy.install_root, foreign.owner, foreign.home, foreign.portable_root
            ));
        }
    }
    portable
}

pub(super) fn listed(names: &[String]) -> String {
    match names {
        [] => "none".to_string(),
        names => names.join(", "),
    }
}
