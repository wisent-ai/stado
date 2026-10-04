//! `stado service grants <SERVICE> [--consumer C] [--apply]` — the grants a
//! service's consumers declare, and the minting of them.
//!
//! A consumer's grant used to exist only as flags somebody remembered:
//! `stado service grant-sync brama --host <H> --consumer
//! oko-model-router-client --capability read:oko-model-router#token
//! --token-file oko-model-router-skarbiec-token`. Nobody remembered them, so
//! grants were issued from the shell instead — dozens in a week, some into
//! the vault replica its owner overwrote within the hour. The missing
//! product side is a consumer declaration that mints grants from the
//! registry.
//!
//! Now the directory carries them beside the consumer it authorizes
//! (`service_directory.services.<service>.consumers.<consumer>.grants`), this
//! reads that declaration, and `--apply` mints every one through the same
//! path `grant-sync` uses. Bare, it prints what apply would do: a plan is the
//! answer to "what is declared", and minting is never the way to ask.

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

/// Add each declared `read:<item>#<field>` to the grant Stado's own reads
/// use, on the vault owner, keeping the bearer in the declared token file
/// there and every capability the grant holds. Only reads can be declared for
/// it; every capability is checked before any is added.
async fn widen_stado_reads(grant: &ConsumerGrant) -> Result<String, CmdError> {
    let mut reads = Vec::new();
    for capability in &grant.capabilities {
        let read = capability
            .strip_prefix("read:")
            .and_then(|rest| rest.split_once('#'))
            .filter(|(item, field)| !item.is_empty() && !field.is_empty());
        let Some(read) = read else {
            return Err(CmdError::click(format!(
                "{capability:?} cannot be added to Stado's own grant: only `read:<item>#<field>` \
                 is, so a declaration never widens what Stado may write"
            )));
        };
        reads.push(read);
    }
    let mut owner = String::new();
    for (item, field) in &reads {
        owner =
            crate::cli::host::ensure_declared_read(&grant.consumer, item, field, &grant.token_file)
                .await?;
    }
    let added: Vec<String> = reads
        .iter()
        .map(|(item, field)| format!("{item}#{field}"))
        .collect();
    Ok(format!(
        "{} may read {} on {owner}",
        grant.consumer,
        added.join(", ")
    ))
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
    // Minted on the host the directory names, not on a host that happens to
    // declare a managed unit for this service: `brama` is placed by a release
    // profile and declares no unit anywhere, so `grant-sync` would answer
    // "brama is not a registry-managed service on <host>" and its consumers
    // could not be minted at all. The directory is this command's
    // input; the host it names is where the vault is.
    let target = host_channel::canonical_target(&host).await.map_err(click)?;
    let runner = production_runner();
    let mut cells = Vec::new();
    let mut failures = Vec::new();
    for item in &declared {
        if item.grant.capabilities.is_empty() {
            return Err(CmdError::click(format!(
                "the declaration for consumer {:?} names no capability; a grant with no \
                 capability authorizes nothing",
                item.grant.consumer
            )));
        }
        // Stado's own consumer holds every read the fleet's deliveries make; a
        // re-mint writes a whole grant and a fresh bearer, so it would replace
        // that list with this one declaration and lock out every host. Its
        // declared reads are added to the grant it holds instead.
        if item.grant.consumer == crate::config::skarbiec_consumer() {
            let (status, detail) = match widen_stado_reads(&item.grant).await {
                Ok(detail) => ("widened", detail),
                Err(error) => {
                    let detail = error
                        .message
                        .clone()
                        .unwrap_or_else(|| "no detail".to_string());
                    failures.push(format!("{}: {detail}", item.grant.consumer));
                    ("refused", detail)
                }
            };
            cells.push(vec![
                item.authorized.clone(),
                item.grant.consumer.clone(),
                host.clone(),
                status.to_string(),
                detail,
            ]);
            continue;
        }
        let audience = item
            .grant
            .audience
            .clone()
            .unwrap_or_else(|| item.grant.consumer.clone());
        let minted = service::remint_consumer_grant_on_host(
            &target,
            &item.grant.consumer,
            &item.grant.capabilities.join(","),
            &item.grant.token_file,
            vault_file,
            ttl_seconds,
            &audience,
            &runner,
        )
        .await
        .map_err(click)?;
        if !minted.succeeded("grant_synced") {
            failures.push(format!("{}: {}", item.grant.consumer, minted.failure()));
        }
        cells.push(vec![
            item.authorized.clone(),
            item.grant.consumer.clone(),
            host.clone(),
            dash(&minted.status),
            dash(&minted.detail),
        ]);
    }
    if as_json {
        print_json(&Value::Array(
            cells
                .iter()
                .map(|row| json!({"authorized": row[0], "consumer": row[1], "host": row[2], "sync": row[3], "detail": row[4], "applied": true}))
                .collect(),
        ))?;
    } else {
        table::print(
            &["AUTHORIZED", "CONSUMER", "HOST", "SYNC", "DETAIL"],
            &cells,
        );
    }
    fail_if_any(&failures, "declared grant")
}
