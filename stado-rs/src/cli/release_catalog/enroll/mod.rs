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
//! 4. for a product with a `runtime`, its rollout policy in the registry,
//!    created when absent (`rollout.rs`);
//! 5. `runtime.grants`: the running service's own consumer, named after the
//!    product, granted exactly those capabilities on the vault owner, and its
//!    bearer delivered to every host the product's release policy rolls out
//!    to (`runtime.rs`).
//!
//! Every step is idempotent: a product that already has what it needs costs
//! one comparison per step and no write.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::cli::host::{grant_item_read, vault_token_sync, write_host_config, TokenSyncMode};
use crate::cli::CmdError;
use crate::release_pipeline::ReleasePipelineManifest;

use super::publisher::{ensure_publisher, fleet_hosts, home_relative};

mod rollout;
mod runtime;

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

    if let Some(runtime) = manifest.runtime.as_ref() {
        steps.push(rollout::ensure_rollout_policy(product, runtime).await?);
        if !runtime.grants.is_empty() {
            steps.push(runtime::ensure_runtime_grant(product, &runtime.grants).await?);
        }
    }

    let python = python_steps(manifest);
    if !python.is_empty() {
        return Err(CmdError::click(format!(
            "{product}: {} run Python, which this workshop does not use; give the product a \
             build and delivery in its own language or Stado's packaging",
            python.join(", ")
        )));
    }

    let untested = untested_platforms(manifest);
    steps.push(json!({ "step": "tests", "untested_required_platforms": untested }));
    Ok(Enrollment { steps, untested })
}

/// Every quality, build, test and delivery step whose program is Python, by
/// name. las and echo built and delivered through `python3 release/*.py`
/// long after the workshop removed Python, and enrolment passed them.
fn python_steps(manifest: &ReleasePipelineManifest) -> Vec<String> {
    let python = |argv: &[String]| {
        argv.first().is_some_and(|program| {
            let name = program.rsplit('/').next().unwrap_or(program);
            name == "python" || name.starts_with("python3")
        })
    };
    let mut found = Vec::new();
    for (platform_name, platform) in &manifest.platforms {
        for gate in platform.quality.iter().chain(&platform.tests) {
            if python(&gate.argv) {
                found.push(format!("{platform_name} step {}", gate.name));
            }
        }
        if python(&platform.build.argv) {
            found.push(format!("{platform_name} build"));
        }
    }
    for delivery in &manifest.deliveries {
        if python(&delivery.argv) {
            found.push(format!("delivery {}", delivery.name));
        }
    }
    found
}

/// Every step whose program is a file in the checkout that the checkout does
/// not hold: the path the step runs, directly (`scripts/x.sh`) or through an
/// interpreter (`bash release/x.sh`). codespy, growth-tactics and OpenEnv
/// kept running `scripts/stado_release.py` for three weeks after the
/// scripts directories were removed, and no build of them could start.
pub(crate) fn missing_step_programs(
    manifest: &ReleasePipelineManifest,
    root: &std::path::Path,
) -> Vec<String> {
    let program = |argv: &[String]| -> Option<String> {
        let first = argv.first()?;
        let named = if first.contains('/') {
            first
        } else {
            argv.get(1)
                .filter(|word| word.contains('/') && !word.starts_with('-'))?
        };
        (!named.starts_with('/') && !root.join(named).exists()).then(|| named.clone())
    };
    let mut missing = Vec::new();
    for (platform_name, platform) in &manifest.platforms {
        for gate in platform.quality.iter().chain(&platform.tests) {
            if let Some(path) = program(&gate.argv) {
                missing.push(format!("{platform_name} step {} runs {path}", gate.name));
            }
        }
        if let Some(path) = program(&platform.build.argv) {
            missing.push(format!("{platform_name} build runs {path}"));
        }
        // The packager reads stage keys inside WISENT_OUTPUT_DIR, which is
        // `<source>/.wisent-output`; a key that names that directory again
        // points one level too deep, and the build fails at packaging after
        // it has compiled everything.
        for key in platform.stage.keys() {
            if let Some(inside) = key.strip_prefix(".wisent-output/") {
                missing.push(format!(
                    "{platform_name} stages {key}, which the packager reads inside \
                     WISENT_OUTPUT_DIR (<source>/.wisent-output) as .wisent-output/{key}; \
                     stage {inside} instead"
                ));
            }
        }
    }
    for delivery in &manifest.deliveries {
        if let Some(path) = program(&delivery.argv) {
            missing.push(format!("delivery {} runs {path}", delivery.name));
        }
    }
    missing
}

/// The refusal for [`missing_step_programs`]: nothing is enrolled or written.
pub(crate) fn missing_programs_refusal(product: &str, missing: &[String]) -> Result<(), CmdError> {
    if missing.is_empty() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{product}: {} — nothing was enrolled; point each step at a Stado command or a file \
         the checkout holds, and each stage key at a path inside WISENT_OUTPUT_DIR",
        missing.join("; ")
    )))
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
            TokenSyncMode::Install,
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
