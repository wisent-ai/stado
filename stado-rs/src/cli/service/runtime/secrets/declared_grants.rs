//! `stado service grants <SERVICE> [--consumer C] [--apply]` — the grants a
//! service's consumers declare, and the minting of them.
//!
//! A consumer's grant used to exist only as flags somebody remembered:
//! `stado service grant-sync brama --host <H> --consumer
//! oko-model-router-client --capability read:oko-model-router#token
//! --token-file oko-model-router-skarbiec-token`. Nobody remembered them, so
//! grants were issued from the shell instead — 26 in the week of 2026-09-12,
//! one of them into the vault replica its owner overwrote within the hour —
//! and Oko's judge named the missing product side on 2026-09-20: "deklaracja
//! konsumentów mintująca granty automatycznie z rejestru".
//!
//! Now the directory carries them beside the consumer it authorizes
//! (`service_directory.services.<service>.consumers.<consumer>.grants`), this
//! reads that declaration, and `--apply` mints every one through the same
//! path `grant-sync` uses. Bare, it prints what apply would do: a plan is the
//! answer to "what is declared", and minting is never the way to ask.

use super::grant::{grant_sync, GrantSyncOptions};
use super::*;
use crate::targets::{ConsumerGrant, ServiceConsumer};

/// The vault and the lifetime the command's own flags default to are declared
/// in its spec beside `grant-sync`'s, so the two stay the same answer to the
/// same question and a declaration never repeats them.
pub(crate) struct DeclaredGrantsOptions<'a> {
    pub(crate) name: &'a str,
    pub(crate) consumer: Option<&'a str>,
    pub(crate) vault_file: &'a str,
    pub(crate) ttl_seconds: u64,
    pub(crate) apply: bool,
    pub(crate) as_json: bool,
}

/// One declared grant, with the consumer of the service it belongs to.
struct Declared {
    authorized: String,
    grant: ConsumerGrant,
}

/// Every grant the directory declares for this service, in consumer order.
async fn declared_grants(
    name: &str,
    only: Option<&str>,
) -> Result<(String, Vec<Declared>), CmdError> {
    let registry = host_channel::canonical_registry().await.map_err(click)?;
    let service = registry.service(name).ok_or_else(|| {
        let mut names: Vec<&str> = registry
            .service_directory
            .as_ref()
            .map(|directory| {
                directory
                    .services
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        names.sort_unstable();
        CmdError::click(format!(
            "the service directory carries no service {name:?}; services there: {}",
            names.join(", ")
        ))
    })?;
    let mut consumers: Vec<(&String, &ServiceConsumer)> = service.consumers.iter().collect();
    consumers.sort_by(|left, right| left.0.cmp(right.0));
    if let Some(only) = only {
        if !consumers
            .iter()
            .any(|(authorized, _)| authorized.as_str() == only)
        {
            let names: Vec<&str> = consumers.iter().map(|(name, _)| name.as_str()).collect();
            return Err(CmdError::click(format!(
                "{name} does not authorize consumer {only:?}; consumers there: {}",
                names.join(", ")
            )));
        }
    }
    let mut out = Vec::new();
    for (authorized, consumer) in consumers {
        if only.is_some_and(|only| only != authorized.as_str()) {
            continue;
        }
        for grant in &consumer.grants {
            out.push(Declared {
                authorized: authorized.clone(),
                grant: grant.clone(),
            });
        }
    }
    Ok((service.active_host.clone(), out))
}

pub(crate) async fn declared_grant_reconcile(
    options: DeclaredGrantsOptions<'_>,
) -> Result<(), CmdError> {
    let DeclaredGrantsOptions {
        name,
        consumer,
        vault_file,
        ttl_seconds,
        apply,
        as_json,
    } = options;
    let (host, declared) = declared_grants(name, consumer).await?;
    if declared.is_empty() {
        return Err(CmdError::click(format!(
            "no consumer of {name} declares a grant, so there is nothing to mint. A product that \
             reads a credential declares it at \
             `service_directory.services.{name}.consumers.<consumer>.grants`: the exact Skarbiec \
             consumer, its capabilities and the owner-only token file. Write one with \
             `stado registry set --path service_directory.services.{name}.consumers.<consumer>.grants --value <JSON>`."
        )));
    }
    if !apply {
        let mut cells = Vec::new();
        for item in &declared {
            cells.push(vec![
                item.authorized.clone(),
                item.grant.consumer.clone(),
                item.grant.capabilities.join(","),
                item.grant.token_file.clone(),
                item.grant
                    .audience
                    .clone()
                    .unwrap_or_else(|| item.grant.consumer.clone()),
            ]);
        }
        if as_json {
            let rows: Vec<Value> = declared
                .iter()
                .map(|item| {
                    json!({
                        "authorized": item.authorized,
                        "consumer": item.grant.consumer,
                        "capabilities": item.grant.capabilities,
                        "token_file": item.grant.token_file,
                        "audience": item.grant.audience.clone().unwrap_or_else(|| item.grant.consumer.clone()),
                        "host": host,
                        "applied": false,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&rows)?);
            return Ok(());
        }
        println!(
            "{name} on {host}: {} declared grant(s); nothing was minted",
            declared.len()
        );
        table::print(
            &[
                "authorized",
                "consumer",
                "capabilities",
                "token file",
                "audience",
            ],
            &cells,
        );
        println!("mint them with `stado service grants {name} --apply`");
        return Ok(());
    }
    for item in &declared {
        if item.grant.capabilities.is_empty() {
            return Err(CmdError::click(format!(
                "the declaration for consumer {:?} names no capability; a grant with no \
                 capability authorizes nothing",
                item.grant.consumer
            )));
        }
        grant_sync(GrantSyncOptions {
            name,
            host: &host,
            consumer: &item.grant.consumer,
            capabilities: &item.grant.capabilities,
            token_file: &item.grant.token_file,
            vault_file,
            ttl_seconds,
            audience: item.grant.audience.as_deref(),
            as_json,
        })
        .await?;
    }
    Ok(())
}
