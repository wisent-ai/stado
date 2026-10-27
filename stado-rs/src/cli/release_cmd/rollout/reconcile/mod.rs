//! Converging one target on its desired release, resolving the executable
//! that release installed, and restoring the previous desired version.

use chrono::Utc;
use serde_json::json;

use crate::cli::CmdError;
use crate::release_control;

use super::{ReleaseActiveBinaryArgs, ReleaseAgentArgs, ReleaseRollbackArgs};

mod declared;
use declared::declared_binary;

pub(in crate::cli::release_cmd) async fn agent(args: &ReleaseAgentArgs) -> Result<(), CmdError> {
    let states =
        crate::release_agent::reconcile_once(&args.target, args.product.as_deref()).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&states)?);
    } else {
        for state in states {
            let active = match state.active.as_ref() {
                Some(record) => record.version.clone(),
                None => "(none active)".to_string(),
            };
            println!(
                "{} target={} generation={} phase={:?} active={active} detail={}",
                state.product, state.target, state.rollout_generation, state.phase, state.detail
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
        .map_err(CmdError::from)?;
    if let Some(notice) = notice {
        eprintln!("{notice}");
    }
    let hostname = crate::providers::vast::system_hostname();
    let local = registry
        .lookup_self(&hostname)
        .map_err(CmdError::from)?
        .ok_or_else(|| {
            CmdError::click(format!("host {hostname} is not in the target registry"))
                .stating(crate::primitives::failure::FailureCode::NotFound)
        })?;
    // `--target` names the entry to reconcile; without it the entry is this
    // host's own, the one the check below requires anyway.
    let target_name = match args.target.as_deref() {
        Some(target) => target,
        None => local.name.as_str(),
    };
    let target_entry = registry
        .targets
        .iter()
        .find(|target| target.name == target_name)
        .ok_or_else(|| CmdError::refused(format!("unknown registry target {target_name:?}")))?;
    if !crate::deploy::host_channel::target_is_this_host(target_entry) {
        return Err(CmdError::refused(format!(
            "target {target_name:?} is not this host; release active binary resolves local release state only"
        )));
    }

    let document = crate::cli::resolver::canonical_document_or_last_good(target_name).await?;
    release_control::validate_registry_contract(&document).map_err(CmdError::declaration)?;
    let control = release_control::control(&document)?.ok_or_else(|| {
        CmdError::click("registry.release_control is not configured")
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    // A product release control does not declare at all (the vault's own
    // Skarbiec) is run under the host's own declaration exactly like one that
    // declares no target here, so it gets the same answer. Refusing it as an
    // unknown release product kept Weles on the vault host from finding its
    // Skarbiec, and Weles does not start without one.
    let Some(policy) = control.products.get(&args.product) else {
        return declared_binary(&args.product, target_entry, target_name, args.json).await;
    };
    let Some(target) = policy.targets.get(target_name) else {
        return declared_binary(&args.product, target_entry, target_name, args.json).await;
    };
    let active =
        crate::release_agent::active_binary(&args.product, target_name, policy, target).await?;
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
    let mut control = release_control::control(&document)?.ok_or_else(|| {
        CmdError::click("registry.release_control is not configured")
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    let Some(policy) = control.products.get_mut(&args.product) else {
        return Err(crate::cli::release_cmd::unknown_release_product(
            &control,
            &args.product,
        ));
    };
    let restored = policy
        .previous
        .take()
        .ok_or_else(|| CmdError::refused("release has no previous desired version to restore"))?;
    policy.previous = policy.desired.take();
    let desired = policy.desired.insert(restored);
    // The restored release gets a rollout generation past both the one it
    // had and the one it replaces, so every host sees a new rollout; the
    // registry's own generation counts this commit.
    let mut rollout_generation = match policy.previous.as_ref() {
        Some(release) => release.rollout_generation.max(desired.rollout_generation),
        None => desired.rollout_generation,
    };
    crate::primitives::tally::Tally::tally(&mut rollout_generation);
    desired.rollout_generation = rollout_generation;
    desired.promoted_at = Utc::now().to_rfc3339();
    let version = desired.version.clone();
    crate::primitives::tally::Tally::tally(&mut control.generation);
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
