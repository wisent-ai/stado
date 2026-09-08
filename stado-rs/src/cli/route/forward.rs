//! Declared forward markers: listing, opening and closing them.

use serde_json::{json, Value};

use super::directory::{directory, endpoint, parsed_registry, selected_target, service, target};
use crate::cli::{registry, CmdError};
use crate::deploy::{host_channel, host_forward};

pub async fn list(as_json: bool) -> Result<(), CmdError> {
    let document = registry::fetch_document().await?;
    let directory = directory(&document)?;
    let mut rows = Vec::with_capacity(directory.services.len());
    for name in directory.services.keys() {
        let declared = service(&directory, name)?;
        let endpoints = declared
            .endpoints
            .into_iter()
            .flat_map(|endpoints| endpoints.iter())
            .map(|(target, endpoint)| {
                json!({
                    "target": target,
                    "url": endpoint.get("url").and_then(Value::as_str),
                })
            })
            .collect::<Vec<_>>();
        let open =
            host_forward::read_local(name).map_err(|error| CmdError::click(error.to_string()))?;
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
    for row in rows {
        println!(
            "{}  authority={}  command={:?}  active={}",
            row["service"].as_str().unwrap_or_default(),
            row["authority"]["target"].as_str().unwrap_or_default(),
            row["authority"]["command"].as_str().unwrap_or_default(),
            row["active_host"].as_str().unwrap_or_default(),
        );
        for endpoint in row["endpoints"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            println!(
                "  {} -> {}",
                endpoint["target"].as_str().unwrap_or_default(),
                endpoint["url"].as_str().unwrap_or("(not declared)"),
            );
        }
        match &row["local_forward"] {
            Value::Null => println!("  local forward: closed"),
            marker => println!(
                "  local forward: open -> {} ({})",
                marker["url"].as_str().unwrap_or_default(),
                marker["marker"].as_str().unwrap_or_default(),
            ),
        }
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
        host_forward::open_local(name, url).map_err(|error| CmdError::click(error.to_string()))?
    } else {
        let registry = parsed_registry(&document)?;
        let target = target(&registry, target_name)?;
        host_forward::open_remote(target, name, url)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?
    };
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
        println!(
            "{}: open on {} -> {} ({})",
            name,
            report["forward"]["location"].as_str().unwrap_or_default(),
            url,
            report["forward"]["marker"].as_str().unwrap_or_default(),
        );
    }
    Ok(())
}

pub async fn close(name: &str, requested_target: Option<&str>) -> Result<(), CmdError> {
    let document = registry::fetch_document().await?;
    let directory = directory(&document)?;
    let declared = service(&directory, name)?;
    let target_name = selected_target(&declared, requested_target);
    endpoint(&declared, target_name)?;

    let local_removed =
        host_forward::close_local(name).map_err(|error| CmdError::click(error.to_string()))?;
    let mut remote_removed = false;
    if !local_removed {
        let registry = parsed_registry(&document)?;
        let target = target(&registry, target_name)?;
        if !host_channel::target_is_this_host(target) {
            remote_removed = host_forward::close_remote(target, name)
                .await
                .map_err(|error| CmdError::click(error.to_string()))?;
        }
    }
    if !local_removed && !remote_removed {
        return Err(CmdError::click(format!(
            "{name} has no open forward marker; run `stado route open {name} --local` or `stado route open {name} --remote` first"
        )));
    }
    println!(
        "{name}: closed {} forward; no marker remains",
        if local_removed { "local" } else { "remote" }
    );
    Ok(())
}
