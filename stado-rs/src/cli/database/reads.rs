//! The plane's reads: the placement-aware listing of every declaration, and
//! the per-consumer resolution that hands out the credential coordinate.

use serde_json::json;

use crate::cli::CmdError;
use crate::service_resolution;

use super::{declared_databases, directory_routes, registry_document};

pub(super) async fn list(json_output: bool) -> Result<(), CmdError> {
    let databases = declared_databases()?;
    if databases.is_empty() {
        if json_output {
            println!("[]");
        }
        return Ok(());
    }
    let document = registry_document().await?;
    let routes = document
        .get("service_directory")
        .is_some()
        .then(|| directory_routes(&document))
        .transpose()?;

    let mut rows = Vec::new();
    // The person-readable lines are written from the declarations the rows
    // are built from, so no declared field is printed as an empty string.
    let mut lines = Vec::new();
    for (name, database) in databases {
        let route = routes.and_then(|routes| routes.get(name));
        lines.push(format!(
            "{name} engine={} item={} scopes=[{}] consumers=[{}] placed={}",
            database.engine(),
            database.item(),
            database.scopes().join(","),
            database.consumers().join(","),
            route.is_some()
        ));
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
        for line in &lines {
            println!("{line}");
        }
    }
    Ok(())
}

pub(super) async fn resolve(name: &str, consumer: &str, json_output: bool) -> Result<(), CmdError> {
    let databases = declared_databases()?;
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

    let uri = format!("stado://database/{name}");
    let mut report = json!({
        "database": uri,
        "engine": database.engine(),
        "scopes": database.scopes(),
        "credential_item": database.item(),
        "placed": placed,
    });
    if placed {
        let resolved =
            service_resolution::resolve(&document, name, consumer).map_err(CmdError::from)?;
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
            uri,
            database.engine(),
            database.item(),
            database.scopes().join(","),
            placed,
        );
    }
    Ok(())
}
