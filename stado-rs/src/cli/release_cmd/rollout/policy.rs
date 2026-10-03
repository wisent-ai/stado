//! Applying reviewed product policy without touching the active release.

use serde_json::json;

use crate::cli::CmdError;
use crate::release_control;

use super::{
    ReleasePolicyApplyArgs, ReleasePolicyDocument, ReleasePolicyRemoveArgs,
    ReleasePolicyTargetRemoveArgs,
};

pub(in crate::cli::release_cmd) async fn apply_policy(
    args: &ReleasePolicyApplyArgs,
) -> Result<(), CmdError> {
    let bytes = std::fs::read(&args.file)?;
    let mut declaration: ReleasePolicyDocument = serde_json::from_slice(&bytes)?;
    if declaration.policy.desired.is_some() || declaration.policy.previous.is_some() {
        return Err(CmdError::click(
            "rollout policy cannot set desired or previous release state; use release promote",
        ));
    }
    let (document, expected_generation) = crate::cli::registry::fetch_versioned_document().await?;
    let mut control = release_control::control(&document)?
        .ok_or_else(|| CmdError::click("registry.release_control is not configured"))?;
    if let Some(current) = control.products.get(&declaration.product) {
        declaration.policy.desired = current.desired.clone();
        declaration.policy.previous = current.previous.clone();
    }
    control
        .products
        .insert(declaration.product.clone(), declaration.policy);
    control.generation = control.generation.saturating_add(1);
    let mut updated = document;
    updated[release_control::RELEASE_CONTROL_KEY] = serde_json::to_value(&control)?;
    release_control::validate_registry_contract(&updated).map_err(CmdError::click)?;
    let store_generation =
        crate::cli::registry::push_document_if(&updated, &expected_generation).await?;
    let report = json!({
        "product": declaration.product,
        "release_control_generation": control.generation,
        "store_generation": store_generation,
        "status": "applied",
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "applied rollout policy for {} at release-control generation {}",
            report["product"].as_str().unwrap_or_default(),
            control.generation
        );
    }
    Ok(())
}

/// `stado release policy-target-remove PRODUCT --target HOST`: the product is
/// no longer released to HOST. One verified registry write removes the target
/// from the product's rollout policy; HOST's release agent then retires the
/// proxy and release processes it ran for the product
/// (`processes::handover::retire_untargeted`). The last target is refused: a
/// policy that rolls out nowhere is not a policy.
pub(in crate::cli::release_cmd) async fn remove_policy_target(
    args: &ReleasePolicyTargetRemoveArgs,
) -> Result<(), CmdError> {
    let (document, expected_generation) = crate::cli::registry::fetch_versioned_document().await?;
    let mut control = release_control::control(&document)?
        .ok_or_else(|| CmdError::refused("registry.release_control is not configured"))?;
    let known = control
        .products
        .keys()
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let policy = control.products.get_mut(&args.product).ok_or_else(|| {
        CmdError::refused(format!(
            "{} has no rollout policy; products with one: {known}",
            args.product
        ))
    })?;
    if !policy.targets.contains_key(&args.target) {
        return Err(CmdError::refused(format!(
            "{} is not released to {}; its targets: {}",
            args.product,
            args.target,
            policy
                .targets
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    if policy.targets.len() == 1 {
        return Err(CmdError::refused(format!(
            "{} is the last target of {}; a rollout policy needs at least one, so stop releasing \
             the product by release control altogether with `stado release policy-remove {}`",
            args.target, args.product, args.product
        )));
    }
    policy.targets.remove(&args.target);
    control.generation = control.generation.saturating_add(1);
    let mut updated = document;
    updated[release_control::RELEASE_CONTROL_KEY] = serde_json::to_value(&control)?;
    release_control::validate_registry_contract(&updated).map_err(CmdError::click)?;
    let store_generation =
        crate::cli::registry::push_document_if(&updated, &expected_generation).await?;
    let report = json!({
        "product": args.product,
        "removed_target": args.target,
        "release_control_generation": control.generation,
        "store_generation": store_generation,
        "status": "removed",
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{} is no longer released to {} (release-control generation {}); its release agent \
             there retires the proxy and release processes it ran for it",
            args.product, args.target, control.generation
        );
    }
    Ok(())
}

/// `stado release policy-remove PRODUCT`: the product is no longer rolled out
/// by release control anywhere. One verified registry write removes its
/// policy, and each former target's release agent then retires the proxy and
/// release processes it ran for the product. The validator refuses the write
/// while anything in the registry still names the product as
/// release-controlled.
pub(in crate::cli::release_cmd) async fn remove_policy(
    args: &ReleasePolicyRemoveArgs,
) -> Result<(), CmdError> {
    let (document, expected_generation) = crate::cli::registry::fetch_versioned_document().await?;
    let mut control = release_control::control(&document)?
        .ok_or_else(|| CmdError::refused("registry.release_control is not configured"))?;
    let known = control
        .products
        .keys()
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let removed = control.products.remove(&args.product).ok_or_else(|| {
        CmdError::refused(format!(
            "{} has no rollout policy; products with one: {known}",
            args.product
        ))
    })?;
    control.generation = control.generation.saturating_add(1);
    let mut updated = document;
    updated[release_control::RELEASE_CONTROL_KEY] = serde_json::to_value(&control)?;
    release_control::validate_registry_contract(&updated).map_err(CmdError::click)?;
    let store_generation =
        crate::cli::registry::push_document_if(&updated, &expected_generation).await?;
    let targets: Vec<String> = removed.targets.keys().cloned().collect();
    let report = json!({
        "product": args.product,
        "former_targets": targets,
        "release_control_generation": control.generation,
        "store_generation": store_generation,
        "status": "removed",
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{} is no longer rolled out by release control (release-control generation {}); \
             the release agents on {} retire the proxy and release processes they ran for it",
            args.product,
            control.generation,
            targets.join(", ")
        );
    }
    Ok(())
}
