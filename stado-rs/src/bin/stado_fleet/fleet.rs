//! Fleet — a named set of registry targets.
//!
//! The codebase always talked about "the fleet" in prose; this module makes
//! one fleet an explicit thing: an entry in the optional top-level `fleets`
//! section of the canonical registry document, with membership declared by
//! the target's own `fleet` field (one target, at most one fleet). The
//! section is additive — documents without it simply have no fleets, and
//! older readers ignore both the section and the field.

use serde_json::Value;

/// One named fleet and its resolved member target names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fleet {
    pub name: String,
    pub notes: String,
    pub members: Vec<String>,
}

/// Fleet names follow the same lowercase-identifier shape registry target
/// names use (`is_target_name` in the targets module, kept private there).
fn is_fleet_name(value: &str) -> bool {
    let alnum = |b: &u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    let bytes = value.as_bytes();
    let (Some(first), Some(last)) = (bytes.first(), bytes.last()) else {
        return false;
    };
    alnum(first)
        && alnum(last)
        && bytes
            .iter()
            .all(|b| alnum(b) || matches!(b, b'.' | b'_' | b'-'))
}

fn target_name(target: &Value) -> Option<&str> {
    target
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.is_empty())
}

/// Parse the optional `fleets` section and resolve membership from the
/// targets' `fleet` fields. Refused, with the exact location named:
/// a non-array section, a malformed entry, a duplicate name, a non-string
/// `fleet` field, or a target pointing at a fleet that was never declared.
pub fn parse_fleets(document: &Value) -> Result<Vec<Fleet>, String> {
    let mut fleets: Vec<Fleet> = Vec::new();
    if let Some(section) = document.get("fleets") {
        let entries = section
            .as_array()
            .ok_or_else(|| "registry.fleets: must be an array".to_string())?;
        for (index, entry) in entries.iter().enumerate() {
            let location = format!("registry.fleets[{index}]");
            let object = entry
                .as_object()
                .ok_or_else(|| format!("{location}: must be an object"))?;
            let name = object
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| is_fleet_name(name))
                .ok_or_else(|| {
                    format!("{location}.name: must be a lowercase fleet identifier")
                })?;
            if fleets.iter().any(|fleet| fleet.name == name) {
                return Err(format!("{location}.name: duplicate fleet '{name}'"));
            }
            let notes = object
                .get("notes")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            fleets.push(Fleet {
                name: name.to_string(),
                notes,
                members: Vec::new(),
            });
        }
    }
    let targets = document
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| "registry.targets: must be an array".to_string())?;
    for (index, target) in targets.iter().enumerate() {
        let location = format!("registry.targets[{index}]");
        let Some(fleet_field) = target.get("fleet") else {
            continue;
        };
        let fleet_name = fleet_field
            .as_str()
            .ok_or_else(|| format!("{location}.fleet: must be a string"))?;
        let name = target_name(target)
            .ok_or_else(|| format!("{location}.name: must be a non-empty string"))?;
        let Some(fleet) = fleets.iter_mut().find(|fleet| fleet.name == fleet_name) else {
            return Err(format!(
                "{location}.fleet: target '{name}' points at undeclared fleet '{fleet_name}'"
            ));
        };
        fleet.members.push(name.to_string());
    }
    Ok(fleets)
}

/// Look one fleet up by name.
pub fn find_fleet<'a>(fleets: &'a [Fleet], name: &str) -> Option<&'a Fleet> {
    fleets.iter().find(|fleet| fleet.name == name)
}

/// `stado_fleet list` — every declared fleet with its members, from the
/// canonical registry document.
pub async fn list(as_json: bool) -> Result<bool, String> {
    let document = stado::cli::registry::fetch_document()
        .await
        .map_err(|exc| exc.to_string())?;
    let fleets = parse_fleets(&document)?;
    if as_json {
        let document: Value = serde_json::json!({
            "fleets": fleets.iter().map(|fleet| serde_json::json!({
                "name": fleet.name,
                "notes": fleet.notes,
                "members": fleet.members,
            })).collect::<Vec<_>>(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&document).map_err(|exc| exc.to_string())?
        );
    } else if fleets.is_empty() {
        println!("no fleets declared in the registry");
    } else {
        for fleet in &fleets {
            let members = if fleet.members.is_empty() {
                "(no members)".to_string()
            } else {
                fleet.members.join(", ")
            };
            println!("{}\t{}", fleet.name, members);
            if !fleet.notes.is_empty() {
                println!("  {}", fleet.notes);
            }
        }
    }
    Ok(true)
}

/// `stado_fleet status NAME` — live state of one fleet's members: health
/// beacons and capacity broadcasts from the store, nothing else.
pub async fn status(name: &str) -> Result<bool, String> {
    let document = stado::cli::registry::fetch_document()
        .await
        .map_err(|exc| exc.to_string())?;
    let fleets = parse_fleets(&document)?;
    let fleet = find_fleet(&fleets, name)
        .ok_or_else(|| format!("fleet '{name}' is not declared in the registry"))?;
    let store = stado::queue::JobStorage::new()
        .await
        .map_err(|exc| exc.to_string())?;
    let consumers = stado::queue::capacity::read_consumer_capacity(&store)
        .await
        .map_err(|exc| exc.to_string())?;
    let broadcasting: Vec<String> = consumers.keys().cloned().collect();
    println!("fleet '{}' — {}", fleet.name, fleet.notes);
    if fleet.members.is_empty() {
        println!("  (no members)");
    }
    for member in &fleet.members {
        match stado::monitor::host_health::load_host_health(&store, member).await {
            Ok(report) => {
                let reported_at = report
                    .beacon
                    .get("reported_at")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                println!("  {member}: last beacon at {reported_at}");
            }
            Err(exc) => println!("  {member}: no readable health beacon ({exc})"),
        }
    }
    if broadcasting.is_empty() {
        println!("  no consumer is broadcasting capacity");
    } else {
        println!("  broadcasting consumers: {}", broadcasting.join(", "));
    }
    Ok(true)
}
