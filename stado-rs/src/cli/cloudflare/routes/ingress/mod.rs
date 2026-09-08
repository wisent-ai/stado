//! Tunnel ingress rules: the rule list as read, the hostnames it declares for
//! one zone, and the two edits that keep exactly one catch-all rule last.

use serde_json::{json, Value};

use crate::cli::cloudflare::api::belongs_to_zone;
use crate::cli::CmdError;

mod constants;

use constants::CATCH_ALL_SERVICE;

pub(super) fn ingress_rules(config: &Value) -> Result<&[Value], CmdError> {
    if !config.is_object() {
        return Err(CmdError::click(
            "Cloudflare tunnel configuration is not an object",
        ));
    }
    match config.get("ingress") {
        None => Ok(&[]),
        Some(Value::Array(rules)) => Ok(rules),
        Some(_) => Err(CmdError::click(
            "Cloudflare tunnel configuration ingress is not an array",
        )),
    }
}

pub(super) fn configured_hostnames(config: &Value, zone: &str) -> Result<Vec<String>, CmdError> {
    let mut hostnames: Vec<String> = ingress_rules(config)?
        .iter()
        .filter_map(|rule| rule.get("hostname").and_then(Value::as_str))
        .filter(|hostname| belongs_to_zone(hostname, zone))
        .map(str::to_string)
        .collect();
    hostnames.sort_unstable();
    hostnames.dedup();
    Ok(hostnames)
}

pub(super) fn route_ingress(
    config: &mut Value,
    hostname: &str,
    origin: &str,
) -> Result<(), CmdError> {
    if !config.is_object() {
        return Err(CmdError::click(
            "Cloudflare tunnel configuration is not an object",
        ));
    }
    let object = config.as_object_mut().expect("object checked above");
    let ingress = object
        .entry("ingress")
        .or_insert_with(|| Value::Array(Vec::new()));
    let rules = ingress.as_array_mut().ok_or_else(|| {
        CmdError::click("Cloudflare tunnel configuration ingress is not an array")
    })?;

    let matching: Vec<usize> = rules
        .iter()
        .enumerate()
        .filter_map(|(index, rule)| {
            (rule.get("hostname").and_then(Value::as_str) == Some(hostname)).then_some(index)
        })
        .collect();
    if matching.len() > 1 {
        return Err(CmdError::click(format!(
            "Cloudflare tunnel configuration contains duplicate ingress rules for {hostname:?}"
        )));
    }
    let mut route = matching
        .first()
        .map(|index| rules.remove(*index))
        .unwrap_or_else(|| json!({ "hostname": hostname, "originRequest": {} }));
    let route_object = route.as_object_mut().ok_or_else(|| {
        CmdError::click(format!(
            "Cloudflare ingress rule for {hostname:?} is not an object"
        ))
    })?;
    route_object.insert("hostname".to_string(), Value::String(hostname.to_string()));
    route_object.insert("service".to_string(), Value::String(origin.to_string()));

    let catch_all_indices: Vec<usize> = rules
        .iter()
        .enumerate()
        .filter_map(|(index, rule)| rule.get("hostname").is_none().then_some(index))
        .collect();
    if catch_all_indices.len() > 1 {
        return Err(CmdError::click(
            "Cloudflare tunnel configuration contains more than one catch-all ingress rule",
        ));
    }
    let catch_all = catch_all_indices
        .first()
        .map(|index| rules.remove(*index))
        .unwrap_or_else(|| json!({ "service": CATCH_ALL_SERVICE }));
    rules.push(route);
    rules.push(catch_all);
    Ok(())
}

pub(super) fn remove_route_ingress(config: &mut Value, hostname: &str) -> Result<usize, CmdError> {
    if !config.is_object() {
        return Err(CmdError::click(
            "Cloudflare tunnel configuration is not an object",
        ));
    }
    let object = config.as_object_mut().expect("object checked above");
    let Some(ingress) = object.get_mut("ingress") else {
        return Ok(0);
    };
    let rules = ingress.as_array_mut().ok_or_else(|| {
        CmdError::click("Cloudflare tunnel configuration ingress is not an array")
    })?;
    let before = rules.len();
    rules.retain(|rule| rule.get("hostname").and_then(Value::as_str) != Some(hostname));
    let removed = before - rules.len();
    if removed == 0 {
        return Ok(0);
    }

    let catch_all_indices: Vec<usize> = rules
        .iter()
        .enumerate()
        .filter_map(|(index, rule)| rule.get("hostname").is_none().then_some(index))
        .collect();
    if catch_all_indices.len() > 1 {
        return Err(CmdError::click(
            "Cloudflare tunnel configuration contains more than one catch-all ingress rule",
        ));
    }
    let catch_all = catch_all_indices
        .first()
        .map(|index| rules.remove(*index))
        .unwrap_or_else(|| json!({ "service": CATCH_ALL_SERVICE }));
    rules.push(catch_all);
    Ok(removed)
}
