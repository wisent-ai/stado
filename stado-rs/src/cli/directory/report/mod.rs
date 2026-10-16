//! What the directory says, printed: the whole block, the placement profiles
//! behind it, and the forward markers this machine carries because of it.

use serde_json::Value;

use crate::observations;
use crate::targets;

use crate::cli::CmdError;

use crate::cli::directory::document::{directory, read_document, services};

pub(in crate::cli::directory) mod declared;
mod markers;
pub(in crate::cli::directory) mod publish;

/// Every declared service, its placement, and the address each host is handed
/// -- each address followed by when anyone last confirmed it answers.
///
/// The endpoint and its freshness are printed on one line on purpose. Read
/// alone, `from <host>: <address>` is a claim with no author and no date;
/// `never` beside it says the fleet has no evidence for the line it just
/// printed.
pub(in crate::cli::directory) async fn show(as_json: bool) -> Result<(), CmdError> {
    let document = read_document().await?;
    let block = directory(&document)?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(block)?);
        return Ok(());
    }
    let all = services(block)?;
    let seen = observations::load();
    for (name, entry) in all {
        match entry.get("active_host").and_then(Value::as_str) {
            Some(active) => println!("{name}  active_host={active}"),
            None => println!("{name}  active_host: none declared"),
        }
        if let Some(endpoints) = entry.get("endpoints").and_then(Value::as_object) {
            for (target, endpoint) in endpoints {
                let url = match endpoint.get("url").and_then(Value::as_str) {
                    Some(url) => url.to_string(),
                    None => "declares no url".to_string(),
                };
                // Keyed by the host the address is written for, not by the
                // host serving: reachability is a property of the pair, and
                // one vantage answering says nothing about the others. The
                // look may have dialed another address for the pair (a
                // resolver adapter), so what it saw is printed with its age.
                let fact = observations::service_fact(name, target);
                let freshness = observations::freshness_in(&seen, &fact);
                let observed = match &freshness {
                    observations::Freshness::Seen(row) => format!(
                        "observed {} from {}: {}, {}",
                        observations::render(&freshness),
                        row.vantage,
                        row.state,
                        row.detail
                    ),
                    observations::Freshness::Never => "observed never".to_string(),
                };
                println!("    from {target}: {url}  [{observed}]");
            }
        }
        if let Some(consumers) = entry.get("consumers").and_then(Value::as_object) {
            let names: Vec<&str> = consumers.keys().map(String::as_str).collect();
            println!("    consumers: {}", names.join(", "));
        }
    }
    Ok(())
}

const PROFILES_KEY: &str = "placement_profiles";

/// Print every placement profile: which services it covers, the order they
/// start and stop in, the state it requires, and which hosts declare units for
/// it.
///
/// Read-only on purpose. A profile decides where services belong across the
/// fleet, and editing that from a per-service command would put a
/// fleet-shaped decision behind a service-shaped verb.
pub(in crate::cli::directory) async fn profiles(as_json: bool) -> Result<(), CmdError> {
    let document = read_document().await?;
    let declared = document
        .get(PROFILES_KEY)
        .and_then(Value::as_array)
        .ok_or_else(|| {
            CmdError::declaration(format!(
                "the registry at {} carries no {PROFILES_KEY}",
                targets::registry_location()
            ))
        })?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(declared)?);
        return Ok(());
    }
    for (index, profile) in declared.iter().enumerate() {
        let name = profile.get("name").and_then(Value::as_str).ok_or_else(|| {
            CmdError::declaration(format!(
                "{PROFILES_KEY}[{index}] in the registry at {} declares no name",
                targets::registry_location()
            ))
        })?;
        println!("{name}");
        for (label, key) in [
            ("services", "services"),
            ("start", "start_order"),
            ("stop", "stop_order"),
        ] {
            if let Some(values) = profile.get(key).and_then(Value::as_array) {
                let names: Vec<&str> = values.iter().filter_map(Value::as_str).collect();
                println!("    {label}: {}", names.join(", "));
            }
        }
        if let Some(hosts) = profile.get("hosts").and_then(Value::as_object) {
            for (host, entry) in hosts {
                let units = match entry.get("units").and_then(Value::as_object) {
                    Some(units) => units.keys().cloned().collect::<Vec<_>>().join(", "),
                    None => "declares no units".to_string(),
                };
                println!("    on {host}: {units}");
            }
        }
        // Required state is what a migration has to carry with the service;
        // naming it here is cheaper than discovering it during a cutover.
        if let Some(state) = profile.get("state").and_then(Value::as_array) {
            let required: Vec<&str> = state
                .iter()
                .filter(|entry| entry.get("required").and_then(Value::as_bool) == Some(true))
                .filter_map(|entry| entry.get("path").and_then(Value::as_str))
                .collect();
            if !required.is_empty() {
                println!("    required state: {}", required.join(", "));
            }
        }
    }
    Ok(())
}
