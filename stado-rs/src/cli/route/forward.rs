//! Declared forward markers: listing, opening and closing them.

use serde_json::{json, Value};

use super::directory::{directory, endpoint, parsed_registry, selected_target, service, target};
use crate::cli::{registry, CmdError};
use crate::deploy::{host_access::forward, host_channel};

pub async fn list(as_json: bool) -> Result<(), CmdError> {
    let document = registry::fetch_document().await?;
    let directory = directory(&document)?;
    let mut rows = Vec::with_capacity(directory.services.len());
    // The person-readable form, written from the same declared values as the
    // JSON rows rather than read back out of them, so a field the directory
    // declares is never printed as an empty string.
    let mut lines = Vec::with_capacity(directory.services.len());
    for name in directory.services.keys() {
        let declared = service(&directory, name)?;
        let mut endpoints = Vec::new();
        lines.push(format!(
            "{name}  authority={}  command={:?}  active={}",
            directory.authority.target, directory.authority.command, declared.active_host
        ));
        for (target, endpoint) in declared.endpoints.into_iter().flatten() {
            let url = endpoint.get("url").and_then(Value::as_str);
            lines.push(match url {
                Some(url) => format!("  {target} -> {url}"),
                None => format!("  {target} -> (not declared)"),
            });
            endpoints.push(json!({ "target": target, "url": url }));
        }
        let open = forward::read_local(name).map_err(CmdError::from)?;
        lines.push(match &open {
            None => "  local forward: closed".to_string(),
            Some(marker) => format!(
                "  local forward: open -> {} ({})",
                marker.url, marker.marker
            ),
        });
        rows.push(json!({
            "service": name,
            "authority": &directory.authority,
            "active_host": declared.active_host,
            "endpoints": endpoints,
            "local_forward": open,
        }));
    }
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "authority": directory.authority,
                "services": rows,
            }))?
        );
        return Ok(());
    }
    for line in lines {
        println!("{line}");
    }
    Ok(())
}

pub async fn open(
    name: &str,
    requested_target: Option<&str>,
    local: bool,
    remote: bool,
    as_json: bool,
) -> Result<(), CmdError> {
    if !local && !remote {
        return Err(CmdError::usage(
            "route open requires --local or --remote; choose where the declared forward marker must live",
        ));
    }
    let document = registry::fetch_document().await?;
    let directory = directory(&document)?;
    let declared = service(&directory, name)?;
    let target_name = selected_target(&declared, requested_target);
    let url = endpoint(&declared, target_name)?;
    let marker = if local {
        forward::open_local(name, url).map_err(CmdError::from)?
    } else {
        let registry = parsed_registry(&document)?;
        let target = target(&registry, target_name)?;
        forward::open_remote(target, name, url)
            .await
            .map_err(CmdError::from)?
    };
    let text = format!(
        "{name}: open on {} -> {url} ({})",
        marker.location, marker.marker
    );
    let report = json!({
        "service": name,
        "authority": directory.authority,
        "active_host": declared.active_host,
        "endpoint": url,
        "forward": marker,
        "status": "open",
    });
    if as_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{text}");
    }
    Ok(())
}

pub async fn close(name: &str, requested_target: Option<&str>) -> Result<(), CmdError> {
    let document = registry::fetch_document().await?;
    let directory = directory(&document)?;
    let declared = service(&directory, name)?;
    let target_name = selected_target(&declared, requested_target);
    endpoint(&declared, target_name)?;

    let local_removed = forward::close_local(name).map_err(CmdError::from)?;
    let mut remote_removed = false;
    if !local_removed {
        let registry = parsed_registry(&document)?;
        let target = target(&registry, target_name)?;
        if !host_channel::target_is_this_host(target) {
            remote_removed = forward::close_remote(target, name)
                .await
                .map_err(CmdError::from)?;
        }
    }
    if !local_removed && !remote_removed {
        return Err(CmdError::click(format!(
            "{name} has no open forward marker; run `stado route open {name} --local` or `stado route open {name} --remote` first"
        ))
        .stating(crate::primitives::failure::FailureCode::NotFound));
    }
    println!(
        "{name}: closed {} forward; no marker remains",
        if local_removed { "local" } else { "remote" }
    );
    Ok(())
}
