//! `stado network down`: leave Stado's network. The registry first makes the
//! host's first alternate path preferred again and drops the node, so no
//! caller is sent to a node about to stop; then the node's unit is removed.
//! The key stays: a later `up` declares the same identity. Running it on a
//! host that is not on the network changes nothing and says so.

use serde_json::json;

use crate::cli::registry::commit_document;
use crate::cli::CmdError;
use crate::network::node::{declaration, State};

pub(super) async fn down(host: Option<&str>, json_output: bool) -> Result<(), CmdError> {
    let registry = crate::targets::fetch_registry_remote()
        .await
        .map_err(|error| CmdError::unreachable(format!("`network down` takes the node out of the registry, whose authority did not answer: {error}")))?;
    let host = super::this_host(&registry, host)?;
    let target = registry
        .lookup(&host)
        .cloned()
        .ok_or_else(|| CmdError::missing(format!("the registry declares no host {host}")))?;
    let primary = std::sync::Arc::new(std::sync::Mutex::new(None));
    let recorded = std::sync::Arc::clone(&primary);
    let withdrawn_host = host.clone();
    let generation = commit_document(move |document| {
        let mut next = document.clone();
        let now = declaration::withdraw(&mut next, &withdrawn_host).map_err(CmdError::refused)?;
        if let Ok(mut slot) = recorded.lock() {
            *slot = now;
        }
        Ok(next)
    })
    .await?;
    if let Err(error) = crate::targets::fetch_registry_remote().await {
        eprintln!(
            "stado network: the registry copy was not refreshed after the path change: {error}"
        );
    }
    let unit = super::unit::remove(&target).await?;
    State::remove().map_err(CmdError::declaration)?;
    let primary = primary.lock().ok().and_then(|mut slot| slot.take());
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "host": host,
                "primary": primary,
                "unit_removed": unit,
                "generation": generation,
            }))?
        );
    } else {
        match &primary {
            Some(primary) => println!("{host} left the Stado network; its preferred path is {primary}; generation={generation}"),
            None => println!("{host} was not on the Stado network; its registry paths are unchanged"),
        }
        println!("  node unit removed: {unit}");
    }
    Ok(())
}
