//! `stado network status`: this host's node and every peer the registry
//! declares, each asked now over the network itself. A peer is `reached`
//! with the way that carried the question (an endpoint or a relay) or
//! `unreached` with every way's answer. The peers are asked by this command's
//! own process, not by the node: a node left executing a Stado image an
//! install has since replaced was refused every LAN peer while this command
//! reached them all, so the node's image is compared with the installed file
//! and a replaced one is named. The command exits non-zero when this host's
//! own node is not serving, or serves a replaced image, after printing what
//! it found.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::network::node::{serving, State};
use crate::network::{key, peers};

pub(super) async fn status(json_output: bool) -> Result<(), CmdError> {
    let registry = peers::copy().await.map_err(CmdError::unreachable)?;
    let state = State::load().map_err(CmdError::declaration)?;
    let published = serving::read().map_err(CmdError::declaration)?;
    let own_key = key::load().ok().map(|keypair| keypair.public_base64());
    let node_serving = published.as_ref().is_some_and(|serving| {
        serving.state == serving::SERVING
            && state
                .as_ref()
                .is_some_and(|state| state.port == serving.port)
            && crate::providers::local::helpers::pid_alive(serving.pid as i32)
    });
    let replaced = match (&published, node_serving) {
        (Some(node), true) => replaced_image(node.pid),
        _ => Ok(None),
    };
    let this_host = state.as_ref().map(|state| state.host.clone());
    let declared: Vec<(String, crate::network::NetworkNode)> = registry
        .targets
        .iter()
        .filter_map(|target| {
            target
                .network
                .clone()
                .map(|node| (target.name.clone(), node))
        })
        .collect();
    let probes = declared.iter().map(|(host, node)| {
        let host = host.clone();
        let relay = node.relay.is_some();
        let endpoints = node.endpoints.clone();
        async move {
            let answer = crate::network::probe(&host).await;
            let mut row = json!({"host": host, "relay": relay, "endpoints": endpoints});
            match answer {
                Ok(reached) => {
                    row["state"] = json!("reached");
                    row["path"] = json!(reached.path);
                    row["through"] = json!(reached.through);
                }
                Err(cause) => {
                    row["state"] = json!("unreached");
                    row["error"] = json!(cause);
                }
            }
            row
        }
    });
    let peers: Vec<Value> = if own_key.is_some() {
        futures::future::join_all(probes).await
    } else {
        Vec::new()
    };
    let undeclared: Vec<&str> = registry
        .targets
        .iter()
        .filter(|target| target.network.is_none())
        .map(|target| target.name.as_str())
        .collect();
    let report = json!({
        "host": this_host,
        "public_key": own_key,
        "port": state.as_ref().map(|state| state.port),
        "node": published,
        "serving": node_serving,
        "replaced_image": replaced.as_ref().ok().cloned().flatten(),
        "image_unread": replaced.as_ref().err(),
        "peers": peers,
        "not_on_network": undeclared,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        match (&this_host, &published) {
            (Some(host), Some(node)) => println!(
                "this host: {host}, node pid {} on port {} ({})",
                node.pid,
                node.port,
                if node_serving {
                    match &replaced {
                        Ok(None) => "serving".to_string(),
                        Ok(Some(program)) => format!(
                            "serving, but on a Stado image an install has since replaced at \
                             {program}; `stado release local converge` restarts it"
                        ),
                        Err(cause) => format!(
                            "serving; whether it executes the installed Stado cannot be read: {cause}"
                        ),
                    }
                } else {
                    format!(
                        "not serving: {}",
                        node.reason
                            .as_deref()
                            .map_or(node.state.as_str(), |reason| reason)
                    )
                }
            ),
            (Some(host), None) => println!("this host: {host}, its node has published nothing"),
            (None, _) => {
                println!("this host has not joined the Stado network; `stado network up` joins it")
            }
        }
        if !peers.is_empty() {
            println!("  peers, as this command reaches them:");
        }
        for row in &peers {
            let host = row["host"].as_str().map_or("?", |host| host);
            match row["state"].as_str() {
                Some("reached") => println!(
                    "  {host}: reached {} via {}",
                    row["path"].as_str().map_or("", |path| path),
                    row["through"].as_str().map_or("", |through| through)
                ),
                _ => println!(
                    "  {host}: unreached: {}",
                    row["error"].as_str().map_or("", |error| error)
                ),
            }
        }
        if !undeclared.is_empty() {
            println!("  not on the network: {}", undeclared.join(", "));
        }
    }
    if let (true, Ok(Some(program)), Some(host)) = (node_serving, &replaced, &this_host) {
        return Err(CmdError::unreachable(format!(
            "{host}'s Stado network node executes a Stado image an install has since replaced at \
             {program}, and macOS refuses such a process the local network while it allows the \
             installed file; `stado release local converge` restarts it"
        )));
    }
    if node_serving {
        Ok(())
    } else {
        Err(CmdError::unreachable(match this_host {
            Some(host) => format!("{host}'s Stado network node is not serving; `stado network up` starts it and says why it cannot"),
            None => "this host has not joined the Stado network; `stado network up` joins it".to_string(),
        }))
    }
}

/// The program node `pid` was started as, when the file there is no longer
/// the image the node executes (an install placed another file there since
/// the node started); none while it is.
fn replaced_image(pid: u32) -> Result<Option<String>, String> {
    let program = crate::deploy::service::process_arguments(pid)?
        .into_iter()
        .next()
        .map(std::path::PathBuf::from)
        .ok_or_else(|| format!("node pid {pid} has no program in its arguments"))?;
    let running = crate::deploy::service::running_images(&[pid])?
        .remove(&pid)
        .ok_or_else(|| format!("the image node pid {pid} executes could not be read"))?;
    let (installed, _) = crate::deploy::service::installed_image(&program)
        .map_err(|error| format!("{} cannot be read: {error}", program.display()))?;
    Ok((!running.is_same_file(&installed)).then(|| program.display().to_string()))
}
