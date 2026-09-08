//! What the directory says, printed: the whole block, the placement profiles
//! behind it, and the forward markers this machine carries because of it.

use serde_json::Value;

use crate::observations;
use crate::targets;

use crate::cli::registry;
use crate::cli::CmdError;

use crate::cli::directory::document::{click, directory, services};

mod markers;
pub(in crate::cli::directory) mod publish;

/// Every declared service, its placement, and the address each host is handed
/// -- each address followed by when anyone last confirmed it answers.
///
/// The endpoint and its freshness are printed on one line on purpose. Read
/// alone, `from operator-host: http://127.0.0.1:8080` is a claim with no
/// author and no date, and that is the exact rendering an operator believed
/// for twelve days while the laptop it named was closed. `never` beside it
/// says the fleet has no evidence for the line it just printed.
pub(in crate::cli::directory) async fn show(as_json: bool) -> Result<(), CmdError> {
    let document = registry::fetch_document().await?;
    let block = directory(&document)?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(block)?);
        return Ok(());
    }
    let all = services(block)?;
    let seen = observations::load();
    for (name, entry) in all {
        let active = entry
            .get("active_host")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        println!("{name}  active_host={active}");
        if let Some(endpoints) = entry.get("endpoints").and_then(Value::as_object) {
            for (target, endpoint) in endpoints {
                let url = endpoint
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or("(no url)");
                // Keyed by the host the address is written for, not by the
                // host serving: reachability is a property of the pair, and
                // one vantage answering says nothing about the others.
                let observed =
                    observations::describe_in(&seen, &observations::service_fact(name, target));
                println!("    from {target}: {url}  [observed {observed}]");
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
    let document = registry::fetch_document().await?;
    let declared = document
        .get(PROFILES_KEY)
        .and_then(Value::as_array)
        .ok_or_else(|| {
            click(format!(
                "the registry at {} carries no {PROFILES_KEY}",
                targets::registry_location()
            ))
        })?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(declared)?);
        return Ok(());
    }
    for profile in declared {
        let name = profile
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("(unnamed)");
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
                let units = entry
                    .get("units")
                    .and_then(Value::as_object)
                    .map(|units| units.keys().cloned().collect::<Vec<_>>().join(", "))
                    .unwrap_or_else(|| "(no units)".to_string());
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
