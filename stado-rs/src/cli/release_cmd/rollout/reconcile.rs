//! Converging one target on its desired release, resolving the executable
//! that release installed, and restoring the previous desired version.

use chrono::Utc;
use serde_json::json;

use crate::cli::CmdError;
use crate::release_control;

use super::{ReleaseActiveBinaryArgs, ReleaseAgentArgs, ReleaseRollbackArgs};

pub(in crate::cli::release_cmd) async fn agent(args: &ReleaseAgentArgs) -> Result<(), CmdError> {
    let product = args.product.as_deref();
    let states = if args.once {
        crate::release_agent::reconcile_once(&args.target, product)
            .await
            .map_err(CmdError::click)?
    } else {
        return crate::release_agent::agent(&args.target, product, false, args.interval_seconds)
            .await
            .map_err(CmdError::click);
    };
    if args.json {
        println!("{}", serde_json::to_string_pretty(&states)?);
    } else {
        for state in states {
            println!(
                "{} target={} generation={} phase={:?} active={} detail={}",
                state.product,
                state.target,
                state.rollout_generation,
                state.phase,
                state
                    .active
                    .as_ref()
                    .map(|record| record.version.as_str())
                    .unwrap_or("-"),
                state.detail
            );
        }
    }
    Ok(())
}

pub(in crate::cli::release_cmd) async fn active_binary(
    args: &ReleaseActiveBinaryArgs,
) -> Result<(), CmdError> {
    let (registry, notice) = crate::targets::fetch_registry_or_last_good()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if let Some(notice) = notice {
        eprintln!("{notice}");
    }
    let hostname = crate::providers::vast::system_hostname();
    let local = registry
        .lookup_self(&hostname)
        .map_err(|error| CmdError::click(error.to_string()))?
        .ok_or_else(|| CmdError::click(format!("host {hostname} is not in the target registry")))?;
    let target_name = args.target.as_deref().unwrap_or(&local.name);
    let target_entry = registry
        .targets
        .iter()
        .find(|target| target.name == target_name)
        .ok_or_else(|| CmdError::click(format!("unknown registry target {target_name:?}")))?;
    if !crate::deploy::host_channel::target_is_this_host(target_entry) {
        return Err(CmdError::click(format!(
            "target {target_name:?} is not this host; active-binary resolves local release state only"
        )));
    }

    let document = crate::cli::resolver::canonical_document_or_last_good(target_name).await?;
    release_control::validate_registry_contract(&document).map_err(CmdError::click)?;
    let control = release_control::control(&document)?
        .ok_or_else(|| CmdError::click("registry.release_control is not configured"))?;
    let policy = control
        .products
        .get(&args.product)
        .ok_or_else(|| CmdError::click(format!("unknown release product {:?}", args.product)))?;
    let target = policy.targets.get(target_name).ok_or_else(|| {
        CmdError::click(format!(
            "release product {:?} has no target {target_name:?}",
            args.product
        ))
    })?;
    let active = crate::release_agent::active_binary(&args.product, target_name, policy, target)
        .map_err(CmdError::click)?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "state": "active",
                "product": args.product,
                "target": target_name,
                "version": active.version,
                "platform": active.platform,
                "artifact_sha256": active.artifact_sha256,
                "manifest_sha256": active.manifest_sha256,
                "path": active.path,
            }))?
        );
    } else {
        println!("{}", active.path.display());
    }
    Ok(())
}

pub(in crate::cli::release_cmd) async fn rollback(
    args: &ReleaseRollbackArgs,
) -> Result<(), CmdError> {
    let (document, expected_generation) = crate::cli::registry::fetch_versioned_document().await?;
    let mut control = release_control::control(&document)?
        .ok_or_else(|| CmdError::click("registry.release_control is not configured"))?;
    let policy = control
        .products
        .get_mut(&args.product)
        .ok_or_else(|| CmdError::click(format!("unknown release product {:?}", args.product)))?;
    let previous = policy
        .previous
        .take()
        .ok_or_else(|| CmdError::click("release has no previous desired version to restore"))?;
    let current = policy.desired.replace(previous);
    policy.previous = current;
    let desired = policy.desired.as_mut().expect("previous installed above");
    desired.rollout_generation =
        policy
            .previous
            .as_ref()
            .map_or(desired.rollout_generation.saturating_add(1), |release| {
                release
                    .rollout_generation
                    .max(desired.rollout_generation)
                    .saturating_add(1)
            });
    desired.promoted_at = Utc::now().to_rfc3339();
    let version = desired.version.clone();
    let rollout_generation = desired.rollout_generation;
    control.generation = control.generation.saturating_add(1);
    let mut updated = document;
    updated[release_control::RELEASE_CONTROL_KEY] = serde_json::to_value(&control)?;
    let stored_generation =
        crate::cli::registry::push_document_if(&updated, &expected_generation).await?;
    let report = json!({
        "product": args.product,
        "version": version,
        "rollout_generation": rollout_generation,
        "registry_generation": control.generation,
        "store_generation": stored_generation,
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "rollback requested product=\
             {} version=\
             {} rollout-generation={}",
            args.product, version, rollout_generation
        );
    }
    Ok(())
}
