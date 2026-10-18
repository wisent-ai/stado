//! Directory-derived inspection and publication: capability, key, placement.

use std::collections::BTreeSet;

use serde_json::json;

use super::directory::{directory, parsed_registry, service, target};
use crate::cli::{placement, registry, CmdError};
use crate::deploy::{host_access::resolver_key, host_capability};

/// A text field of `report` as the report gives it, or which field the
/// report left out, rather than an empty word where it is missing.
fn reported(report: &serde_json::Value, field: &str) -> String {
    match report[field].as_str() {
        Some(value) => value.to_string(),
        None => format!("(no {field} reported)"),
    }
}

pub async fn capability(name: &str, as_json: bool) -> Result<(), CmdError> {
    let document = registry::fetch_document().await?;
    let directory = directory(&document)?;
    let declared = service(&directory, name)?;
    let registry = parsed_registry(&document)?;
    let target = target(&registry, declared.active_host)?;
    let runner = crate::deploy::production_runner();
    let broker =
        host_capability::resolve(target, &host_capability::BrokerFiles::default(), &runner)
            .await
            .map_err(CmdError::from)?;
    let routes = host_capability::routes(target, &broker, &runner)
        .await
        .map_err(CmdError::from)?;
    let report = json!({
        "service": name,
        "authority": directory.authority,
        "active_host": declared.active_host,
        "vault": broker.vault,
        "report": routes,
    });
    if as_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("service:   {name}");
        println!("host:      {}", declared.active_host);
        println!("authority: {}", directory.authority.target);
        println!("vault:     {}", broker.vault);
        // A host report without its route list says nothing about routes;
        // printed as a count of none, it would claim what the host never said.
        let Some(rows) = report["report"]["routes"].as_array() else {
            println!("routes:    not reported by the host: {}", report["report"]);
            return Ok(());
        };
        println!("routes:    {}", rows.len());
        for row in rows {
            println!(
                "  {:<52} {}/{}",
                crate::cli::reporting::field(&row["resource"]),
                crate::cli::reporting::field(&row["item"]),
                crate::cli::reporting::field(&row["field"]),
            );
        }
    }
    Ok(())
}

pub async fn key(target: &str, as_json: bool) -> Result<(), CmdError> {
    let document = registry::fetch_document().await?;
    let directory = directory(&document)?;
    let parsed = parsed_registry(&document)?;
    let report = resolver_key::authorize(&parsed, target)
        .await
        .map_err(CmdError::from)?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "service_directory_authority": directory.authority,
                "resolver_key": report,
            }))?
        );
    } else {
        println!(
            "{}: resolver key {} ({}), authorized_keys on {} {}",
            reported(&report, "target"),
            reported(&report, "key_state"),
            reported(&report, "key_type"),
            reported(&report, "authority"),
            reported(&report, "authorized_keys"),
        );
    }
    Ok(())
}

pub async fn publish_placement(mobile: bool, as_json: bool) -> Result<(), CmdError> {
    let (document, generation) = registry::fetch_versioned_document().await?;
    let directory = directory(&document)?;
    let registry = parsed_registry(&document)?;
    let mut hosts: BTreeSet<&str> = BTreeSet::new();
    for name in directory.services.keys() {
        hosts.insert(service(&directory, name)?.active_host);
    }
    if mobile {
        hosts.retain(|name| {
            registry
                .lookup(name)
                .is_some_and(|target| target.mobile_runtime.is_some())
        });
    }
    if hosts.is_empty() {
        return Err(CmdError::click(if mobile {
            "the service directory has no active host declaring mobile_runtime; add it to the serving target declaration"
        } else {
            "the service directory declares no active hosts; add active_host to service_directory.services entries"
        })
        .stating(crate::primitives::failure::FailureCode::Config));
    }

    let mut published = Vec::with_capacity(hosts.len());
    for host in hosts {
        published
            .push(placement::publish_placement_policy_report(&document, &generation, host).await?);
    }
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "authority": directory.authority,
                "mobile": mobile,
                "published": published,
            }))?
        );
    } else {
        for report in &published {
            println!(
                "{}: published {} at registry generation {}",
                reported(report, "target"),
                match report["actions"].as_array() {
                    Some(actions) => format!("{} actions", actions.len()),
                    None => "(no actions list reported)".to_string(),
                },
                reported(report, "registry_generation"),
            );
        }
    }
    Ok(())
}
