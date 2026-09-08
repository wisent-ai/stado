//! Whole-document validation of the optional placement declarations.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::placement::document::{profiles, transactions};
use crate::placement::validate::{
    validate_identifier, validate_probe, validate_state_path, validate_unit,
};
use crate::placement::{PROFILES_KEY, TRANSACTIONS_KEY};

fn profile_names(document: &Value) -> Result<BTreeSet<String>, String> {
    let targets = document
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| "registry.targets: must be an array".to_string())?;
    Ok(targets
        .iter()
        .filter_map(|target| target.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .collect())
}

/// Validate optional placement declarations in a registry-v2 document.
pub fn validate_registry_contract(document: &Value) -> Result<(), String> {
    let target_names = profile_names(document)?;
    let profiles = profiles(document)?;
    let mut names = BTreeSet::new();
    for (profile_index, profile) in profiles.iter().enumerate() {
        let location = format!("registry.{PROFILES_KEY}[{profile_index}]");
        validate_identifier(&profile.name, &format!("{location}.name"))?;
        if !names.insert(profile.name.clone()) {
            return Err(format!(
                "{location}.name: duplicate placement profile {:?}",
                profile.name
            ));
        }
        if profile.services.is_empty() {
            return Err(format!("{location}.services: must not be empty"));
        }
        let mut service_names = BTreeSet::new();
        for (index, service) in profile.services.iter().enumerate() {
            validate_identifier(service, &format!("{location}.services[{index}]"))?;
            if !service_names.insert(service.clone()) {
                return Err(format!(
                    "{location}.services[{index}]: duplicate service {service:?}"
                ));
            }
        }
        for (field, order) in [
            ("stop_order", &profile.stop_order),
            ("start_order", &profile.start_order),
        ] {
            let ordered: BTreeSet<&String> = order.iter().collect();
            let expected: BTreeSet<&String> = profile.services.iter().collect();
            if order.len() != profile.services.len() || ordered != expected {
                return Err(format!(
                    "{location}.{field}: must contain every profile service exactly once"
                ));
            }
        }
        if profile.hosts.len() < 2 {
            return Err(format!(
                "{location}.hosts: must contain at least two destinations"
            ));
        }
        for (host, host_profile) in &profile.hosts {
            let host_location = format!("{location}.hosts.{host}");
            if !target_names.contains(host) {
                return Err(format!("{host_location}: host is not a registry target"));
            }
            let configured: BTreeSet<&String> = host_profile.units.keys().collect();
            let expected: BTreeSet<&String> = profile.services.iter().collect();
            if configured != expected {
                return Err(format!(
                    "{host_location}.units: must define every profile service exactly once"
                ));
            }
            for (service, unit) in &host_profile.units {
                validate_unit(unit, &format!("{host_location}.units.{service}"))?;
            }
            let mut probed = BTreeSet::new();
            for (index, probe) in host_profile.probes.iter().enumerate() {
                let probe_location = format!("{host_location}.probes[{index}]");
                if !service_names.contains(&probe.service) {
                    return Err(format!(
                        "{probe_location}.service: is not in the placement profile"
                    ));
                }
                if !probed.insert(&probe.service) {
                    return Err(format!(
                        "{probe_location}.service: duplicate probe for {:?}",
                        probe.service
                    ));
                }
                validate_probe(probe, &probe_location)?;
            }
            if probed.len() != service_names.len() {
                return Err(format!(
                    "{host_location}.probes: must probe every profile service"
                ));
            }
        }
        let mut state_paths = BTreeSet::new();
        if profile.state.is_empty() {
            return Err(format!("{location}.state: must not be empty"));
        }
        for (index, state) in profile.state.iter().enumerate() {
            validate_state_path(&state.path, &format!("{location}.state[{index}].path"))?;
            if !state_paths.insert(&state.path) {
                return Err(format!(
                    "{location}.state[{index}].path: duplicate state path {:?}",
                    state.path
                ));
            }
        }
        for (index, route) in profile.routing.iter().enumerate() {
            let route_location = format!("{location}.routing[{index}]");
            if !profile.hosts.contains_key(&route.host) {
                return Err(format!(
                    "{route_location}.host: must be one of the profile hosts"
                ));
            }
            if !profile.hosts.contains_key(&route.active_when_destination) {
                return Err(format!(
                    "{route_location}.active_when_destination: must be one of the profile hosts"
                ));
            }
            if route.unit.managed().is_none() {
                return Err(format!(
                    "{route_location}.unit: placement routing units must be Stado-managed"
                ));
            }
            validate_unit(&route.unit, &format!("{route_location}.unit"))?;
        }
    }

    let transactions = transactions(document)?;
    let mut transaction_profiles = BTreeSet::new();
    for (index, transaction) in transactions.iter().enumerate() {
        let location = format!("registry.{TRANSACTIONS_KEY}[{index}]");
        if uuid::Uuid::parse_str(&transaction.id).is_err() {
            return Err(format!("{location}.id: must be a UUID"));
        }
        if !names.contains(&transaction.profile) {
            return Err(format!(
                "{location}.profile: references an unknown placement profile"
            ));
        }
        if !target_names.contains(&transaction.from_host)
            || !target_names.contains(&transaction.to_host)
            || transaction.from_host == transaction.to_host
        {
            return Err(format!(
                "{location}: must reference two different registry targets"
            ));
        }
        if chrono::DateTime::parse_from_rfc3339(&transaction.started_at).is_err() {
            return Err(format!("{location}.started_at: must be RFC3339"));
        }
        if !transaction_profiles.insert(&transaction.profile) {
            return Err(format!(
                "{location}.profile: another transaction already owns this profile"
            ));
        }
    }
    Ok(())
}
