//! `stado placement holds HOST [--json]`: everything the registry places on
//! one host, read the way an evacuation reads it — profiles (the vault's
//! marked), roles of the host Stado process, the registry authority, fixed
//! routes, the profiles it stands by for, release-control products, pinned
//! schedules, its store's write fence and the profiles whose placement cannot
//! be told — answering "has this host been emptied yet" with nothing changed.
//! The schedule hand-over an evacuation makes and its read-back live in
//! [`schedules`].

mod schedules;

pub(super) use schedules::{repin_schedules, schedules_to_repin};

use serde_json::{json, Value};

use self::schedules::{all_schedules, pinned_schedules, pins_of};
use super::scan::{authority_holder, fixed_routes_on, listed, placed_on, rolled_out_on};
use crate::cli::{registry, CmdError};

/// After an evacuation's steps all answered, the registry itself must say
/// `from` holds no profile and no role of the host Stado process (and, when
/// the authority was moved, that `to` is the authority). A step whose receipt
/// said it moved while the registry still places the service on `from` is
/// refused naming what stands, rather than reported as an evacuation.
pub(super) async fn ensure_emptied(from: &str, to: &str, authority: bool) -> Result<(), CmdError> {
    let document = registry::fetch_document().await?;
    let placed = placed_on(&document, from)?;
    let fixed = fixed_routes_on(&document, from)?;
    let names: Vec<String> = placed.profiles.iter().map(|profile| profile.name.clone()).collect();
    let held_by = authority_holder(&document);
    let mut standing: Vec<String> = Vec::new();
    standing.extend((!names.is_empty()).then(|| format!("profiles {}", names.join(", "))));
    standing.extend((!fixed.roles.is_empty()).then(|| format!("roles {}", fixed.roles.join(", "))));
    if authority && held_by != Some(to) {
        standing.push(format!("the registry authority, which the directory names as {}", crate::cli::reporting::optional_text(held_by)));
    }
    if standing.is_empty() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "every step of the evacuation of {from} answered, but the registry still places {} \
         there; stado placement holds {from} shows it",
        standing.join("; ")
    ))
    .stating(crate::primitives::failure::FailureCode::InfraDown))
}

/// `stado placement holds` with no host: one line per registered host —
/// what it holds, whether it is the authority, the schedules pinned to it
/// (the schedule store read once for every host) and its store's write fence
/// (every host read at once over its channel; one that does not answer is
/// said, not read as unfenced).
pub(in crate::cli::placement) async fn holds_everywhere(json_output: bool) -> Result<(), CmdError> {
    let document = registry::fetch_document().await?;
    crate::targets::validate_registry(&document).map_err(CmdError::from)?;
    let registry_targets = crate::cli::placement::candidates::parse_registry(&document)?;
    let authority = authority_holder(&document);
    let runner = crate::deploy::production_runner();
    let (schedules, fences) = futures::join!(
        all_schedules(),
        futures::future::join_all(registry_targets.targets.iter().map(|target| crate::cli::placement::transfer::store_fence(target, &runner)))
    );
    let mut rows = Vec::new();
    for (target, fence) in registry_targets.targets.iter().zip(fences) {
        let placed = placed_on(&document, &target.name)?;
        let fixed = fixed_routes_on(&document, &target.name)?;
        let profiles: Vec<&str> = placed.profiles.iter().map(|profile| profile.name.as_str()).collect();
        let holds_authority = authority == Some(target.name.as_str());
        let pinned = schedules.as_ref().map_err(String::clone).and_then(|schedules| pins_of(&registry_targets, target, schedules));
        let (pinned, unread) = match pinned {
            Ok(pins) => (json!(pins.into_iter().map(|(id, _)| id).collect::<Vec<_>>()), Value::Null),
            Err(cause) => (Value::Null, json!(cause)),
        };
        let (store_fence, store_fence_unread) = match fence {
            Ok(intent) => (intent, Value::Null),
            Err(cause) => (Value::Null, json!(cause.to_string())),
        };
        rows.push(json!({
            "host": target.name,
            "emptied": profiles.is_empty() && fixed.roles.is_empty() && !holds_authority,
            "authority": holds_authority,
            "profiles": profiles,
            "roles": fixed.roles,
            "fixed_routes": fixed.routes,
            "rolled_out_by": rolled_out_on(&document, &target.name),
            "pinned_schedules": pinned,
            "pinned_schedules_unread": unread,
            "store_fence": store_fence,
            "store_fence_unread": store_fence_unread,
        }));
    }
    if json_output {
        println!("{}", serde_json::to_string_pretty(&json!({ "hosts": rows }))?);
        return Ok(());
    }
    for row in &rows {
        let names = |key: &str| -> Vec<String> {
            row[key].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
        };
        let marker = if row["authority"] == true { " (registry authority)" } else { "" };
        let emptied = if row["emptied"] == true { " (emptied)" } else { "" };
        let schedules = match row["pinned_schedules_unread"].as_str() {
            Some(cause) => format!("unread ({cause})"),
            None => listed(&names("pinned_schedules")),
        };
        println!(
            "{}{marker}{emptied}: profiles {}; roles {}; fixed routes {}; release control {}; pinned schedules {schedules}; \
             store {}",
            crate::cli::reporting::field(&row["host"]),
            listed(&names("profiles")),
            listed(&names("roles")),
            listed(&names("fixed_routes")),
            listed(&names("rolled_out_by")),
            fence_words(&row["store_fence"], &row["store_fence_unread"])
        );
    }
    Ok(())
}

/// A store's write fence as one phrase: `not fenced`, `fenced by <owner>
/// transaction <id>`, or `fence unread (<cause>)`.
fn fence_words(intent: &Value, unread: &Value) -> String {
    match (unread.as_str(), intent.is_null()) {
        (Some(cause), _) => format!("fence unread ({cause})"),
        (None, true) => "not fenced".to_string(),
        (None, false) => format!(
            "fenced by {} transaction {}",
            crate::cli::reporting::field(&intent["owner"]),
            crate::cli::reporting::field(&intent["transaction"])
        ),
    }
}

pub(in crate::cli::placement) async fn holds(host: &str, json_output: bool) -> Result<(), CmdError> {
    let document = registry::fetch_document().await?;
    crate::targets::validate_registry(&document).map_err(CmdError::from)?;
    let registry_targets = crate::cli::placement::candidates::parse_registry(&document)?;
    let target = crate::cli::placement::candidates::target(&registry_targets, host)?;
    // The host's own store fence: a host a role move left keeps its copy
    // fenced, every write there refused; a host that does not answer is said.
    let fence = crate::cli::placement::transfer::store_fence(target, &crate::deploy::production_runner())
        .await
        .map_err(|error| error.to_string());
    let placed = placed_on(&document, host)?;
    let fixed = fixed_routes_on(&document, host)?;
    let authority = authority_holder(&document) == Some(host);
    let vault = crate::cli::secrets::VAULT_SERVICE;
    let profiles: Vec<Value> = placed.profiles.iter().map(|profile| json!({
        "profile": profile.name, "services": profile.services, "vault": profile.services.iter().any(|service| service == vault),
    })).collect();
    let emptied = placed.profiles.is_empty() && fixed.roles.is_empty() && !authority;
    // The profiles that name this host without being placed on it: where it
    // stands by, which is where an evacuation onto it can move them.
    let stands_by: Vec<String> = crate::placement::profiles(&document)
        .map_err(CmdError::declaration)?
        .into_iter()
        .filter(|profile| profile.hosts.contains_key(host))
        .filter(|profile| !placed.profiles.iter().any(|kept| kept.name == profile.name))
        .map(|profile| profile.name)
        .collect();
    let pinned: Result<Vec<String>, String> =
        pinned_schedules(&document, host).await.map(|pinned| pinned.into_iter().map(|(id, _)| id).collect());
    let rolled_out = rolled_out_on(&document, host);
    if json_output {
        let (schedules, unread) = match &pinned {
            Ok(ids) => (json!(ids), Value::Null),
            Err(cause) => (Value::Null, json!(cause)),
        };
        let (store_fence, store_fence_unread) = match &fence {
            Ok(intent) => (intent.clone(), Value::Null),
            Err(cause) => (Value::Null, json!(cause)),
        };
        let report = json!({
            "host": host,
            "emptied": emptied,
            "authority": authority,
            "profiles": profiles,
            "roles": fixed.roles,
            "fixed_routes": fixed.routes,
            "stands_by_for": stands_by,
            "rolled_out_by": rolled_out,
            "pinned_schedules": schedules,
            "pinned_schedules_unread": unread,
            "untold_profiles": placed.untold,
            "store_fence": store_fence,
            "store_fence_unread": store_fence_unread,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    for profile in &placed.profiles {
        let carries = if profile.services.iter().any(|service| service == vault) { " (the fleet vault)" } else { "" };
        println!("profile {}{carries}: {}", profile.name, profile.services.join(", "));
    }
    for role in &fixed.roles {
        println!("role {role} of the host Stado process");
    }
    if authority {
        println!("the registry authority");
    }
    println!("fixed routes: {}", listed(&fixed.routes));
    println!("stands by for: {}", listed(&stands_by));
    println!("release control rolls out here: {}", listed(&rolled_out));
    match &pinned {
        Ok(ids) => println!("schedules pinned here: {}", listed(ids)),
        Err(cause) => println!("schedules pinned here could not be read: {cause}"),
    }
    match &fence {
        Ok(intent) if intent.is_null() => println!("store not fenced"),
        Ok(intent) => println!(
            "store fenced by {} transaction {}: every write there is refused{}",
            crate::cli::reporting::field(&intent["owner"]),
            crate::cli::reporting::field(&intent["transaction"]),
            if intent["owner"].as_str() == Some(crate::cli::storage::FENCE_OWNER) {
                "; a role move leaves the copy it moved fenced so, and a role move back onto this host, or stado storage fence disarm for that transaction, lifts it"
            } else {
                ""
            }
        ),
        Err(cause) => println!("store fence could not be read: {cause}"),
    }
    if !placed.untold.is_empty() {
        println!("placement cannot be told: {}", placed.untold.join("; "));
    }
    if emptied {
        println!("{host} holds no placement profile, no role of the host Stado process and not the registry authority");
        for product in &rolled_out {
            println!("stop releasing {product} here with stado release policy remove-target {product} --target {host}");
        }
    } else {
        println!("empty it with stado placement evacuate --from-host {host} --to-host <host> --reason <why>");
    }
    Ok(())
}
