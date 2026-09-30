//! Adding a product's build secrets to one host's workload secret declaration.
//!
//! Each host keeps its own lists: the vault owner allows items this machine
//! does not, and the other way round. Writing this machine's lists onto the
//! owner replaced the owner's with ours, and the owner's validator refused the
//! result (`weles-admission-api#token names an item absent from
//! agent.skarbiec.items`). So each host's own lists are read, the missing
//! references are added to them, and `items` is written before `secret_fields`:
//! the validator judges every field against the items already on disk.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::cli::host::{remote_config_output, write_host_config, RemoteConfigAction};
use crate::cli::CmdError;
use crate::release_pipeline::ReleasePipelineManifest;

const ITEMS_KEY: &str = "agent.skarbiec.items";
const FIELDS_KEY: &str = "agent.skarbiec.secret_fields";
/// Where `stado config show` reports the two lists it resolved.
const RESOLVED_ITEMS: &str = "/resolved/agent_skarbiec_items";
const RESOLVED_FIELDS: &str = "/resolved/agent_skarbiec_secret_fields";

fn strings(document: &Value, pointer: &str) -> BTreeSet<String> {
    document
        .pointer(pointer)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// Add every `(item, field)` in `missing` to `host`'s own declaration.
pub(super) async fn declare_on_host(
    host: &str,
    missing: &[&(String, String)],
) -> Result<(), CmdError> {
    let target = crate::deploy::host_channel::canonical_target(host)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let shown = remote_config_output(
        &target,
        RemoteConfigAction::Show,
        &crate::deploy::production_runner(),
    )
    .await?;
    let document: Value = serde_json::from_str(&shown).map_err(|error| {
        CmdError::click(format!(
            "{host}: stado config show did not answer JSON: {error}"
        ))
    })?;
    let mut items = strings(&document, RESOLVED_ITEMS);
    let mut fields = strings(&document, RESOLVED_FIELDS);
    let (before_items, before_fields) = (items.len(), fields.len());
    for (item, field) in missing {
        items.insert(item.clone());
        fields.insert(format!("{item}#{field}"));
    }
    // A refusal names the host and the lists it judged: without them one
    // "config unchanged" from two hosts could not be told apart.
    let context = |key: &str, error: CmdError| {
        CmdError::click(format!(
            "{host}: writing {key} failed ({before_items} items and {before_fields} fields \
             read from its stado config show, adding {}): {error}",
            missing
                .iter()
                .map(|(item, field)| format!("{item}#{field}"))
                .collect::<Vec<_>>()
                .join(", ")
        ))
    };
    if items.len() != before_items {
        write_host_config(host, ITEMS_KEY, &json!(items).to_string())
            .await
            .map_err(|error| context(ITEMS_KEY, error))?;
    }
    if fields.len() != before_fields {
        write_host_config(host, FIELDS_KEY, &json!(fields).to_string())
            .await
            .map_err(|error| context(FIELDS_KEY, error))?;
    }
    Ok(())
}

/// One `item#field` reference, refused when it is not one.
fn reference(product: &str, reference: &str) -> Result<(String, String), CmdError> {
    reference
        .split_once('#')
        .filter(|(item, field)| !item.is_empty() && !field.is_empty())
        .map(|(item, field)| (item.to_string(), field.to_string()))
        .ok_or_else(|| {
            CmdError::click(format!(
                "{product}: secret_env reference {reference:?} is not item#field"
            ))
        })
}

/// Every `item#field` the manifest's platforms and deliveries read at build
/// time, refused when one is not an `item#field` reference.
pub(super) fn secret_references(
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
        .map(|each| reference(&manifest.product, each))
        .collect()
}

/// Declare each platform's secrets, and those of the deliveries that run on
/// it, on every registry target that builds that platform.
///
/// The owner and this host were the only hosts declared. On 2026-09-30
/// oko-landing's web build was refused because the one linux-amd64 builder,
/// ubuntu-server-rtx-pro-6000, did not list vercel-deployment#team_id in its
/// agent.skarbiec.secret_fields (78acffb8), and nothing ever added it: a host
/// that builds a platform learns its secrets here, from its own lists.
pub(super) async fn declare_on_builders(
    manifest: &ReleasePipelineManifest,
) -> Result<Value, CmdError> {
    let registry = crate::deploy::host_channel::canonical_registry()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let mut declared = Vec::new();
    for (name, platform) in &manifest.platforms {
        let references = platform
            .secret_env
            .values()
            .chain(
                manifest
                    .deliveries
                    .iter()
                    .filter(|delivery| &delivery.platform == name)
                    .flat_map(|delivery| delivery.secret_env.values()),
            )
            .map(|each| reference(&manifest.product, each))
            .collect::<Result<BTreeSet<_>, _>>()?;
        if references.is_empty() {
            continue;
        }
        let missing: Vec<&(String, String)> = references.iter().collect();
        for target in registry
            .targets
            .iter()
            .filter(|target| target.release_platform == platform.runner_platform)
        {
            declare_on_host(&target.name, &missing).await?;
            declared.push(json!({ "host": target.name, "platform": name }));
        }
    }
    Ok(json!({
        "step": "builder-secrets",
        "product": manifest.product,
        "declared_on": declared,
    }))
}

