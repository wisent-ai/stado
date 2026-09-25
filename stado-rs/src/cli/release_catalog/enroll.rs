//! Everything a product's release manifest needs from the fleet, set up by
//! Stado itself before the product's first build, and checked again before
//! every later one.
//!
//! A new product used to learn what it needed one refusal at a time: no
//! publisher (`release_api.publishers declares no publisher`), a build secret
//! no agent was allowed to read (the job was never claimed), a platform
//! without post-build tests (every task stayed `awaiting_tests`), each
//! repaired by hand by whoever hit it, and a service's own bearer was minted
//! by hand after its first start failed. The manifest states all of it, so
//! this step reads them from it:
//!
//! 1. the release publisher, through `declare_publisher`;
//! 2. every `secret_env` reference a platform or delivery declares, added to
//!    the workload secret declaration on the vault owner and this host and
//!    granted to the workload agent in the owner's vault;
//! 3. the post-build tests each required platform must declare for a build
//!    to qualify a task, reported by name when missing;
//! 4. `runtime.grants`: the running service's own consumer, named after the
//!    product, granted exactly those capabilities on the vault owner, and its
//!    bearer delivered to every host the product's release policy rolls out
//!    to.
//!
//! Every step is idempotent: a product that already has what it needs costs
//! one comparison per step and no write.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::cli::host::{grant_item_read, vault_token_sync, write_host_config};
use crate::cli::{registry, CmdError};
use crate::release_control;
use crate::release_pipeline::ReleasePipelineManifest;

use super::publisher::{ensure_publisher, fleet_hosts, home_relative};

/// The config keys the workload secret gate reads (`config::agent_skarbiec_items`
/// and `config::agent_skarbiec_secret_fields`).
const AGENT_ITEMS_KEY: &str = "agent.skarbiec.items";
const AGENT_SECRET_FIELDS_KEY: &str = "agent.skarbiec.secret_fields";

/// What enrolling one product found and did, step by step.
pub(crate) struct Enrollment {
    pub steps: Vec<Value>,
    /// Required platforms that declare no post-build tests.
    pub untested: Vec<String>,
}

/// Enroll `manifest`'s product: every step below, in order.
pub(crate) async fn enroll(manifest: &ReleasePipelineManifest) -> Result<Enrollment, CmdError> {
    let product = manifest.product.as_str();
    let mut steps = Vec::new();

    ensure_publisher(product).await?;
    steps.push(json!({ "step": "publisher", "product": product }));

    let references = secret_references(manifest)?;
    if !references.is_empty() {
        steps.push(ensure_workload_secrets(product, &references).await?);
    }

    if let Some(runtime) = manifest
        .runtime
        .as_ref()
        .filter(|runtime| !runtime.grants.is_empty())
    {
        steps.push(ensure_runtime_grant(product, &runtime.grants).await?);
    }

    let untested = untested_platforms(manifest);
    steps.push(json!({ "step": "tests", "untested_required_platforms": untested }));
    Ok(Enrollment { steps, untested })
}

/// The bearer file a service's consumer is delivered as, under `~/.stado`.
fn runtime_token_file(product: &str) -> String {
    format!("{product}-skarbiec-token")
}

/// Run this Stado binary with `arguments` and return its stdout, or the
/// refusal it printed.
fn stado(arguments: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .args(arguments)
        .output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// Grant the running service's consumer exactly `grants` on the vault owner
/// and deliver its bearer to every rollout target.
async fn ensure_runtime_grant(product: &str, grants: &[String]) -> Result<Value, CmdError> {
    for grant in grants {
        let well_formed = grant
            .split_once(':')
            .and_then(|(action, rest)| {
                rest.split_once('#')
                    .map(|(item, field)| (action, item, field))
            })
            .is_some_and(|(action, item, field)| {
                !action.is_empty() && !item.is_empty() && !field.is_empty()
            });
        if !well_formed {
            return Err(CmdError::click(format!(
                "{product}: runtime.grants entry {grant:?} is not action:item#field"
            )));
        }
    }
    let (owner, _) = fleet_hosts().await?;
    // The recorded grant lists capabilities as item#field:action.
    let wanted: BTreeSet<String> = grants
        .iter()
        .filter_map(|grant| grant.split_once(':'))
        .map(|(action, reference)| format!("{reference}:{action}"))
        .collect();
    let recorded: BTreeSet<String> = stado(&[
        "credentials",
        "grant",
        "show",
        "--host",
        &owner,
        product,
        "--json",
    ])
    .ok()
    .and_then(|text| serde_json::from_str::<Value>(&text).ok())
    .and_then(|record| {
        record
            .get("capabilities")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
    })
    .unwrap_or_default();
    let token_file = runtime_token_file(product);
    let minted = recorded != wanted;
    if minted {
        let capabilities = grants.join(",");
        stado(&[
            "credentials", "token", "mint", product, "--host", &owner, "--capabilities",
            &capabilities, "--audience", product, "--replace-capabilities",
            "--token-file-name", &token_file,
        ])
        .map_err(|refusal| {
            CmdError::click(format!(
                "{product}: minting its runtime bearer on {owner} with {capabilities} failed: {refusal}"
            ))
        })?;
        eprintln!("{product}: runtime consumer {product} granted {capabilities} on {owner}");
    }

    let (document, _) = registry::fetch_versioned_document().await?;
    let targets: Vec<String> = release_control::control(&document)?
        .and_then(|control| {
            control
                .products
                .get(product)
                .map(|policy| policy.targets.keys().cloned().collect())
        })
        .unwrap_or_default();
    // The copy is idempotent, and repeating it is what repairs a target that
    // missed an earlier delivery or joined the rollout later.
    let path = format!("~/.stado/{token_file}");
    let mut delivered = Vec::new();
    for target in targets.iter().filter(|target| **target != owner) {
        vault_token_sync(&owner, target, product, &path, &path, false, false).await?;
        delivered.push(target.clone());
    }
    Ok(json!({
        "step": "runtime-grant",
        "product": product,
        "consumer": product,
        "granted_on": owner,
        "capabilities": grants,
        "minted": minted,
        "delivered_to": delivered,
        "rollout_targets": targets,
    }))
}

/// Every `item#field` the manifest's platforms and deliveries read at build
/// time, refused when one is not an `item#field` reference.
fn secret_references(
    manifest: &ReleasePipelineManifest,
) -> Result<BTreeSet<(String, String)>, CmdError> {
    manifest
        .platforms
        .values()
        .flat_map(|platform| platform.secret_env.values())
        .chain(
            manifest
                .deliveries
                .iter()
                .flat_map(|delivery| delivery.secret_env.values()),
        )
        .map(|reference| {
            reference
                .split_once('#')
                .filter(|(item, field)| !item.is_empty() && !field.is_empty())
                .map(|(item, field)| (item.to_string(), field.to_string()))
                .ok_or_else(|| {
                    CmdError::click(format!(
                        "{}: secret_env reference {reference:?} is not item#field",
                        manifest.product
                    ))
                })
        })
        .collect()
}

/// Required platforms whose manifest names no post-build test, so none of
/// their builds can ever qualify a task.
fn untested_platforms(manifest: &ReleasePipelineManifest) -> Vec<String> {
    manifest
        .platforms
        .iter()
        .filter(|(_, platform)| platform.required && platform.tests.is_empty())
        .map(|(name, _)| name.clone())
        .collect()
}

/// Declare the product's build secrets for the workload agent and grant them.
async fn ensure_workload_secrets(
    product: &str,
    references: &BTreeSet<(String, String)>,
) -> Result<Value, CmdError> {
    let declared_fields: BTreeSet<&str> = crate::config::agent_skarbiec_secret_fields()
        .iter()
        .map(String::as_str)
        .collect();
    let declared_items: BTreeSet<&str> = crate::config::agent_skarbiec_items()
        .iter()
        .map(String::as_str)
        .collect();
    let missing: Vec<&(String, String)> = references
        .iter()
        .filter(|(item, field)| {
            !declared_fields.contains(format!("{item}#{field}").as_str())
                || !declared_items.contains(item.as_str())
        })
        .collect();
    if missing.is_empty() {
        return Ok(json!({ "step": "workload-secrets", "product": product, "added": [] }));
    }
    let added: Vec<String> = missing
        .iter()
        .map(|(item, field)| format!("{item}#{field}"))
        .collect();

    let consumer = crate::config::agent_skarbiec_consumer();
    let token_file = home_relative(crate::config::agent_skarbiec_token_file());
    if consumer.is_empty() || token_file.is_empty() {
        return Err(CmdError::click(format!(
            "{product} reads build secrets ({}) but this host declares no workload agent \
             (agent.skarbiec.consumer / agent.skarbiec.token_file), so they cannot be granted",
            added.join(", ")
        )));
    }

    let (owner, client) = fleet_hosts().await?;
    let mut fields: BTreeSet<String> = declared_fields
        .iter()
        .map(|entry| entry.to_string())
        .collect();
    let mut items: BTreeSet<String> = declared_items
        .iter()
        .map(|entry| entry.to_string())
        .collect();
    for (item, field) in &missing {
        fields.insert(format!("{item}#{field}"));
        items.insert(item.clone());
    }
    let mut hosts = vec![owner.clone(), client.clone()];
    hosts.dedup();
    for host in &hosts {
        write_host_config(host, AGENT_SECRET_FIELDS_KEY, &json!(fields).to_string()).await?;
        write_host_config(host, AGENT_ITEMS_KEY, &json!(items).to_string()).await?;
    }
    if client != owner {
        vault_token_sync(
            &client,
            &owner,
            consumer,
            &token_file,
            &token_file,
            false,
            false,
        )
        .await?;
    }
    for (item, field) in &missing {
        grant_item_read(&owner, consumer, item, field, &token_file, false).await?;
    }
    eprintln!(
        "{product}: workload agent {consumer} may now read {} (declared on {}, granted on {owner})",
        added.join(", "),
        hosts.join(", ")
    );
    Ok(json!({
        "step": "workload-secrets",
        "product": product,
        "added": added,
        "declared_on": hosts,
        "granted_on": owner,
        "consumer": consumer,
    }))
}
