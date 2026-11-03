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

/// The host's own list at `pointer` of its `stado config show`. A report
/// without it, or with an entry that is not text, is refused: read as an
/// empty list, the write that follows replaced every role and field the host
/// already declared with only the ones being added.
fn strings(host: &str, document: &Value, pointer: &str) -> Result<BTreeSet<String>, CmdError> {
    let refused = |what: String| {
        CmdError::click(format!(
            "{host}: stado config show {what}, so its own list cannot be extended without \
             replacing it; nothing was written"
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    };
    document
        .pointer(pointer)
        .and_then(Value::as_array)
        .ok_or_else(|| refused(format!("carries no list at {pointer}")))?
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| refused(format!("lists {entry} at {pointer}, which is not text")))
        })
        .collect()
}

/// Add every `(role, field)` in `missing` to `host`'s own declaration.
pub(super) async fn declare_on_host(
    host: &str,
    missing: &[&(String, String)],
) -> Result<(), CmdError> {
    let target = crate::cli::canonical_host(host).await?;
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
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    let mut roles = strings(host, &document, RESOLVED_ROLES)?;
    let mut fields = strings(host, &document, RESOLVED_FIELDS)?;
    let (before_roles, before_fields) = (roles.len(), fields.len());
    for (role, field) in missing {
        roles.insert(role.clone());
        fields.insert(format!("{role}#{field}"));
    }
    // A refusal names the host and the lists it judged: without them one
    // "config unchanged" from two hosts could not be told apart.
    let context = |key: &str, error: CmdError| {
        error.within(format!(
            "{host}: writing {key} failed ({before_roles} roles and {before_fields} fields \
             read from its stado config show, adding {})",
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
            CmdError::refused(format!(
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
///
/// A builder whose host channel answers on none of its declared paths is
/// left out and named under `skipped`, with each path's failure: its own
/// lists cannot be read, and refusing the whole submission for it stopped
/// every build of the product while one builder was asleep. A builder left
/// out never publishes the secret fields, so placement does not pin a job
/// that needs them to it. A platform none of whose builders answers is
/// refused, naming each, since no builder could then take its jobs.
pub(super) async fn declare_on_builders(
    manifest: &ReleasePipelineManifest,
) -> Result<Value, CmdError> {
    let registry = crate::deploy::host_channel::canonical_registry()
        .await
        .map_err(CmdError::from)?;
    let runner = crate::deploy::production_runner();
    let mut declared = Vec::new();
    let mut skipped = Vec::new();
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
        let mut unanswered = Vec::new();
        let mut reached = 0usize;
        for target in registry
            .targets
            .iter()
            .filter(|target| target.release_platform == platform.runner_platform)
        {
            if let Some(why) = unanswered_channel(target, &runner).await {
                eprintln!(
                    "{}: builder {} of {name} is left out: {why}",
                    manifest.product, target.name
                );
                unanswered.push(format!("{}: {why}", target.name));
                skipped.push(json!({ "host": target.name, "platform": name, "cause": why }));
                continue;
            }
            declare_on_host(&target.name, &missing).await?;
            declared.push(json!({ "host": target.name, "platform": name }));
            reached += 1;
        }
        if reached == 0 && !unanswered.is_empty() {
            return Err(CmdError::unreachable(format!(
                "{}: no builder of {name} ({}) answers its host channel, so no builder could \
                 take its jobs and nothing was queued: {}",
                manifest.product,
                platform.runner_platform,
                unanswered.join("; ")
            )));
        }
    }
    Ok(json!({
        "step": "builder-secrets",
        "product": manifest.product,
        "declared_on": declared,
        "skipped": skipped,
    }))
}

/// Why `target`'s host channel answers on none of its declared paths, or
/// `None` when one answers (this host itself always does). Every path is
/// probed as `stado host` diagnostics probe it, without sending the
/// operation, so a builder that is asleep is told from one that refuses.
async fn unanswered_channel(
    target: &crate::targets::ComputeTarget,
    runner: &crate::deploy::Runner,
) -> Option<String> {
    let probes = match crate::deploy::host_channel::probe_ssh_connections(target, runner).await {
        Ok(probes) => probes,
        Err(error) => return Some(format!("its host channel could not be probed: {error}")),
    };
    if probes.iter().any(|probe| probe.reachable) {
        return None;
    }
    if probes.is_empty() {
        return Some("it declares no host channel path".to_string());
    }
    Some(format!(
        "none of its host channel paths answers ({})",
        probes
            .iter()
            .map(|probe| format!(
                "{} {}: {}",
                probe.name,
                probe.destination,
                probe.error.as_deref().unwrap_or("no answer")
            ))
            .collect::<Vec<_>>()
            .join("; ")
    ))
}
