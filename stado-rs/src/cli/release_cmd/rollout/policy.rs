//! Applying reviewed product policy without touching the active release.

use serde_json::json;

use crate::cli::CmdError;
use crate::release_control;

use super::{ReleasePolicyApplyArgs, ReleasePolicyDocument};

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
