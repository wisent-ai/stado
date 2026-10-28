//! `stado resolver resolve SERVICE --consumer NAME`: one service resolved
//! for one consumer from the directory as this host reads it.

use std::sync::Arc;

use serde_json::json;

use super::{current_target, directory_document, read_local_snapshot};
use crate::cli::CmdError;
use crate::service_resolution;
use crate::targets::RegistryStore;

fn logical_name(value: &str) -> Result<&str, CmdError> {
    // Both spellings name the service: the bare logical name, and the
    // stado://service/<name> address a workload uses.
    let value = match value.strip_prefix("stado://service/") {
        Some(name) => name,
        None => value,
    }
    .trim();
    if value.is_empty() || value.contains('/') {
        return Err(CmdError::usage(
            "SERVICE must be one logical name or stado://service/<name>",
        ));
    }
    Ok(value)
}

/// The directory as this host reads it ([`directory_document`]) and the
/// service resolved for `consumer` from it, with this host's own target name.
pub(super) async fn resolved_for(
    service: &str,
    consumer: &str,
) -> Result<(service_resolution::ResolvedService, String), CmdError> {
    let service = logical_name(service)?;
    let store = Arc::new(RegistryStore::open().await?);
    let (bootstrap, _, _) = read_local_snapshot(&store).await?;
    let target = current_target(&bootstrap).map_err(CmdError::declaration)?;
    let document = directory_document(store, &bootstrap, &target).await?;
    let resolved =
        service_resolution::resolve(&document, service, consumer).map_err(CmdError::from)?;
    Ok((resolved, target))
}

pub(super) async fn resolve_once(
    service: &str,
    consumer: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let (resolved, _) = resolved_for(service, consumer).await?;
    let uri = format!("stado://service/{}", resolved.name);
    if json_output {
        let report = json!({
            "service": uri,
            "generation": resolved.generation,
            "capabilities": resolved.capabilities,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{uri} generation={} capabilities={}",
            resolved.generation,
            resolved.capabilities.join(",")
        );
    }
    Ok(())
}
