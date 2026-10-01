//! Adding a product's build secrets to one host's workload secret declaration.
//!
//! Each host keeps its own lists: the vault owner allows roles this machine
//! does not, and the other way round, so writing one host's lists onto another
//! would replace them. Each host's own lists are therefore read, the missing
//! references are added to them, and `roles` is written before
//! `secret_fields`: the validator judges every field against the roles already
//! on disk.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::cli::host::{remote_config_output, write_host_config, RemoteConfigAction};
use crate::cli::CmdError;
use crate::release_pipeline::ReleasePipelineManifest;

const ROLES_KEY: &str = "agent.skarbiec.roles";
const FIELDS_KEY: &str = "agent.skarbiec.secret_fields";
/// Where `stado config show` reports the two lists it resolved.
const RESOLVED_ROLES: &str = "/resolved/agent_skarbiec_roles";
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

/// Add every `(role, field)` in `missing` to `host`'s own declaration.
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
    let mut roles = strings(&document, RESOLVED_ROLES);
    let mut fields = strings(&document, RESOLVED_FIELDS);
    let (before_roles, before_fields) = (roles.len(), fields.len());
    for (role, field) in missing {
        roles.insert(role.clone());
        fields.insert(format!("{role}#{field}"));
    }
    // A refusal names the host and the lists it judged: without them one
    // "config unchanged" from two hosts could not be told apart.
    let context = |key: &str, error: CmdError| {
        CmdError::click(format!(
            "{host}: writing {key} failed ({before_roles} roles and {before_fields} fields \
             read from its stado config show, adding {}): {error}",
            missing
                .iter()
                .map(|(role, field)| format!("{role}#{field}"))
                .collect::<Vec<_>>()
                .join(", ")
        ))
    };
    if roles.len() != before_roles {
        write_host_config(host, ROLES_KEY, &json!(roles).to_string())
            .await
            .map_err(|error| context(ROLES_KEY, error))?;
    }
    if fields.len() != before_fields {
        write_host_config(host, FIELDS_KEY, &json!(fields).to_string())
            .await
            .map_err(|error| context(FIELDS_KEY, error))?;
    }
    Ok(())
}

/// One `role#field` reference, refused when it is not one.
fn reference(product: &str, reference: &str) -> Result<(String, String), CmdError> {
    reference
        .split_once('#')
        .filter(|(role, field)| !role.is_empty() && !field.is_empty())
        .map(|(role, field)| (role.to_string(), field.to_string()))
        .ok_or_else(|| {
            CmdError::click(format!(
                "{product}: secret_env reference {reference:?} is not role#field"
            ))
        })
}

/// Every `role#field` the manifest's platforms and deliveries read at build
/// time, refused when one is not a `role#field` reference.
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
/// it, on every registry target that builds that platform, so a host that
/// builds a platform learns its secrets from its own lists.
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
