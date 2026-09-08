//! The plane's reads: the placement-aware listing of every declaration, and
//! the per-consumer resolution that hands out the credential coordinate.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::service_resolution;

use super::{directory_routes, registry_document};

pub(super) async fn list(json_output: bool) -> Result<(), CmdError> {
    let databases = crate::config::database_api_databases()
        .map_err(|problems| CmdError::click(problems.join("; ")))?;
    let document = registry_document().await?;
    let routes = directory_routes(&document)?;

    let mut rows = Vec::new();
    for (name, database) in databases {
        let route = routes.get(name);
        rows.push(json!({
            "database": name,
            "engine": database.engine(),
            "item": database.item(),
            "scopes": database.scopes(),
            "consumers": database.consumers(),
            "placed": route.is_some(),
            "active_host": route.and_then(|route| route.get("active_host")).cloned(),
        }));
    }

    if json_output {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        for row in &rows {
            let scopes = row["scopes"]
                .as_array()
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default();
            let consumers = row["consumers"]
                .as_array()
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default();
            println!(
                "{} engine={} item={} scopes=[{scopes}] consumers=[{consumers}] placed={}",
                row["database"].as_str().unwrap_or_default(),
                row["engine"].as_str().unwrap_or_default(),
                row["item"].as_str().unwrap_or_default(),
                row["placed"]
                    .as_bool()
                    .map(|placed| placed.to_string())
                    .unwrap_or_default(),
            );
        }
    }
    Ok(())
}

pub(super) async fn resolve(name: &str, consumer: &str, json_output: bool) -> Result<(), CmdError> {
    let databases = crate::config::database_api_databases()
        .map_err(|problems| CmdError::click(problems.join("; ")))?;
    let database = databases.get(name).ok_or_else(|| {
        CmdError::usage(format!(
            "unknown database {name:?}; declared: {}",
            databases.keys().cloned().collect::<Vec<_>>().join(", ")
        ))
    })?;
    if !database.allows_consumer(consumer) {
        return Err(CmdError::usage(format!(
            "consumer {consumer:?} is not authorized for database {name:?}"
        )));
    }

    // Placement is optional. A declared database whose service-directory
    // route does not exist yet still resolves: the consumer learns the
    // credential coordinate and takes the endpoint from the credential
    // itself. Requiring a route here would make every fresh declaration
    // unresolvable until someone edited the canonical registry by hand,
    // which is exactly the one-off this plane exists to replace.
    let document = registry_document().await?;
    let placed = document
        .get("service_directory")
        .and_then(|directory| directory.get("services"))
        .and_then(|services| services.get(name))
        .is_some();

    let mut report = json!({
        "database": format!("stado://database/{name}"),
        "engine": database.engine(),
        "scopes": database.scopes(),
        "credential_item": database.item(),
        "placed": placed,
    });
    if placed {
        let resolved =
            service_resolution::resolve(&document, name, consumer).map_err(CmdError::click)?;
        report["generation"] = json!(resolved.generation);
        report["active_host"] = json!(resolved.active_host);
        report["endpoint"] = json!(resolved.endpoint.url);
        report["capabilities"] = json!(resolved.capabilities);
    }
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{} engine={} item={} scopes={} placed={}",
            report["database"].as_str().unwrap_or_default(),
            database.engine(),
            database.item(),
            database.scopes().join(","),
            placed,
        );
    }
    Ok(())
}
