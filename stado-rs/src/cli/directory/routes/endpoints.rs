//! `stado service directory bind` and `... endpoint` — the two answers that
//! come straight out of the declaration: how the placed host should serve, and
//! what address the directory hands the host that is asking.

use serde_json::{json, Value};

use crate::observations;

use crate::cli::registry;
use crate::cli::CmdError;

use crate::cli::directory::document::{click, directory, service, this_target, DIRECTORY_KEY};
use crate::cli::directory::routes::routable_address;

pub(in crate::cli::directory) async fn bind(
    name: &str,
    target: Option<String>,
    as_json: bool,
) -> Result<(), CmdError> {
    let document = registry::fetch_document().await?;
    let block = directory(&document)?;
    let entry = service(block, name)?;
    let asking = match target {
        Some(value) => value,
        None => this_target().await?,
    };
    let active = entry
        .get("active_host")
        .and_then(Value::as_str)
        .filter(|host| !host.is_empty())
        .ok_or_else(|| click(format!("{name} declares no active_host")))?;
    if asking != active {
        return Err(click(format!(
            "{name} is placed on {active}, not on {asking}; only the placed host serves it"
        )));
    }
    let registry = registry::read_registry().await?;
    let placed = registry
        .targets
        .iter()
        .find(|candidate| candidate.name == active)
        .ok_or_else(|| click(format!("{active} is not a host in the registry")))?;
    let bind_address = routable_address(placed).ok_or_else(|| {
        click(format!(
            "{active} carries no address the rest of the fleet can reach it at"
        ))
    })?;
    // Every other host in the registry: the mesh encrypts those hops, and a
    // peer that is not in the registry is not one of ours.
    let peers: Vec<String> = registry
        .targets
        .iter()
        .filter(|candidate| candidate.name != active)
        .filter_map(routable_address)
        .collect();
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "service": name,
                "host": active,
                "bind_address": bind_address,
                "encrypted_peers": peers,
            }))?
        );
    } else {
        // Neutral keys: this verb answers for any service, and the names a
        // given program wants them under are that program's business.
        println!("bind_address={bind_address}");
        println!("encrypted_peers={}", peers.join(","));
    }
    Ok(())
}

pub(in crate::cli::directory) async fn endpoint(
    name: &str,
    target: Option<String>,
    as_json: bool,
) -> Result<(), CmdError> {
    let document = registry::fetch_document().await?;
    let block = directory(&document)?;
    let entry = service(block, name)?;
    let target = match target {
        Some(value) => value,
        None => this_target().await?,
    };
    let active = entry
        .get("active_host")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let url = entry
        .get("endpoints")
        .and_then(Value::as_object)
        .and_then(|endpoints| endpoints.get(&target))
        .and_then(|endpoint| endpoint.get("url"))
        .and_then(Value::as_str);
    // The endpoint and the age of the fleet's evidence for it are one answer,
    // not two. This verb is what scripts and operators use to find out where a
    // service is; handing back an address with no indication that nobody has
    // confirmed it since the machine was last awake is how a valid declaration
    // routed twelve days of work into a closed laptop.
    let observed = observations::describe(&observations::service_fact(name, &target));
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "service": name,
                "target": target,
                "active_host": active,
                "url": url,
                "observed": observed,
            }))?
        );
        return Ok(());
    }
    match url {
        Some(url) => println!(
            "{name} active on {active}, reached from {target} at {url} (observed {observed})"
        ),
        // Not a default and not an error: an undeclared endpoint means nobody
        // has said how this machine reaches the service, and inventing a
        // loopback address here is what sends a client to the wrong process.
        None => {
            println!("{name} active on {active}; {DIRECTORY_KEY} declares no endpoint for {target}")
        }
    }
    Ok(())
}
