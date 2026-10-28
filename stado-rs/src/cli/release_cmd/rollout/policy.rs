//! `stado release policy apply|show|list|remove|target remove|root repair`:
//! the reviewed rollout policy of each release-controlled product, applied
//! without touching the active release and read back in the shape `apply` takes.

mod remove;

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::release_control;

use self::remove::remove_policy;
use super::{
    ReleasePolicyApplyArgs, ReleasePolicyCommands, ReleasePolicyDocument, ReleasePolicyListArgs,
    ReleasePolicyRootCommands, ReleasePolicyShowArgs, ReleasePolicyTargetCommands,
    ReleasePolicyTargetRemoveArgs,
};

pub(in crate::cli::release_cmd) async fn run(
    command: &ReleasePolicyCommands,
) -> Result<(), CmdError> {
    match command {
        ReleasePolicyCommands::Apply(args) => apply_policy(args).await,
        ReleasePolicyCommands::Show(args) => show_policy(args).await,
        ReleasePolicyCommands::List(args) => list_policies(args).await,
        ReleasePolicyCommands::Remove(args) => remove_policy(args).await,
        ReleasePolicyCommands::Target(ReleasePolicyTargetCommands::Remove(args)) => {
            remove_policy_target(args).await
        }
        ReleasePolicyCommands::Root(ReleasePolicyRootCommands::Repair(args)) => {
            super::portable::portable_root(args).await
        }
    }
}

/// The registry's release control, refused by name when it is not configured.
async fn control() -> Result<release_control::ReleaseControl, CmdError> {
    let (document, _generation) = crate::cli::registry::fetch_versioned_document().await?;
    release_control::control(&document)?
        .ok_or_else(|| CmdError::refused("registry.release_control is not configured"))
}

/// `stado release policy show PRODUCT`: `{product, policy}` as the registry
/// holds it. `desired` and `previous` are the release state `promote` keeps;
/// `apply` refuses them, so strip both to re-apply an edited copy.
async fn show_policy(args: &ReleasePolicyShowArgs) -> Result<(), CmdError> {
    let control = control().await?;
    let policy = control.products.get(&args.product).ok_or_else(|| {
        CmdError::refused(format!(
            "{} has no rollout policy; products with one: {}",
            args.product,
            control
                .products
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ))
    })?;
    let document = json!({ "product": args.product, "policy": policy });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&document)?);
    } else {
        println!("product: {}", args.product);
        println!(
            "targets: {}",
            policy
                .targets
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!("policy: {}", serde_json::to_string(&document["policy"])?);
        let foreign = release_control::foreign_targets(policy);
        if let Some(first) = foreign.first() {
            let hosts: Vec<&str> = foreign.iter().map(|target| target.host.as_str()).collect();
            println!(
                "install_root lies under {}'s home, so {} install outside their own; apply refuses it until it reads {}, \
                 which stado release policy root repair {} writes",
                first.owner,
                hosts.join(", "),
                first.portable_root,
                args.product
            );
        }
    }
    Ok(())
}

/// `stado release policy list`: every release-controlled product with its
/// targets and the release it desires.
async fn list_policies(args: &ReleasePolicyListArgs) -> Result<(), CmdError> {
    let control = control().await?;
    let rows: Vec<serde_json::Value> = control
        .products
        .iter()
        .map(|(product, policy)| {
            json!({
                "product": product,
                "targets": policy.targets.keys().collect::<Vec<_>>(),
                "desired": policy.desired,
            })
        })
        .collect();
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({ "generation": control.generation, "products": rows })
            )?
        );
    } else {
        for row in &rows {
            println!("{row}");
        }
    }
    Ok(())
}

async fn apply_policy(args: &ReleasePolicyApplyArgs) -> Result<(), CmdError> {
    let bytes = std::fs::read(&args.file)?;
    let mut declaration: ReleasePolicyDocument = serde_json::from_slice(&bytes)?;
    if declaration.policy.desired.is_some() || declaration.policy.previous.is_some() {
        return Err(CmdError::refused(
            "rollout policy cannot set desired or previous release state; use release promote",
        ));
    }
    // A root spelled under one target's home sends every target with another
    // home outside its own; the policy is refused naming the portable root.
    let foreign = release_control::foreign_targets(&declaration.policy);
    if let Some(first) = foreign.first() {
        let hosts: Vec<String> = foreign
            .iter()
            .map(|target| format!("{} (home {})", target.host, target.home))
            .collect();
        return Err(CmdError::refused(format!(
            "install_root {} lies under {}'s home, so {} would install where they are not at home; write it as {}",
            declaration.policy.install_root,
            first.owner,
            hosts.join(", "),
            first.portable_root
        )));
    }
    let (document, expected_generation) = crate::cli::registry::fetch_versioned_document().await?;
    let mut control = release_control::control(&document)?.ok_or_else(|| {
        CmdError::click("registry.release_control is not configured")
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    if let Some(current) = control.products.get(&declaration.product) {
        declaration.policy.desired = current.desired.clone();
        declaration.policy.previous = current.previous.clone();
    }
    control
        .products
        .insert(declaration.product.clone(), declaration.policy);
    let store_generation = commit(document, &expected_generation, &mut control).await?;
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
            declaration.product, control.generation
        );
    }
    Ok(())
}

/// `stado release policy target remove PRODUCT --target HOST`: the product is
/// no longer released to HOST. One verified registry write removes the target
/// from the product's rollout policy; HOST's release agent then retires the
/// proxy and release processes it ran for the product
/// (`processes::handover::retire_untargeted`). The last target is refused: a
/// policy that rolls out nowhere is not a policy.
async fn remove_policy_target(args: &ReleasePolicyTargetRemoveArgs) -> Result<(), CmdError> {
    let report = drop_policy_target(&args.product, &args.target).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{} is no longer released to {} (release-control generation {}); its release agent \
             there retires the proxy and release processes it ran for it",
            args.product, args.target, report["release_control_generation"]
        );
    }
    Ok(())
}

/// Remove `target` from `product`'s rollout policy in one verified registry
/// write, refusing an unknown product, a host the product is not released
/// to and its only target; the receipt `target remove` prints. Placement's
/// evacuation drops the hosts it empties through this same write.
pub(crate) async fn drop_policy_target(product: &str, target: &str) -> Result<Value, CmdError> {
    let (document, expected_generation) = crate::cli::registry::fetch_versioned_document().await?;
    let mut control = release_control::control(&document)?
        .ok_or_else(|| CmdError::refused("registry.release_control is not configured"))?;
    let known = control
        .products
        .keys()
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let policy = control.products.get_mut(product).ok_or_else(|| {
        CmdError::refused(format!(
            "{product} has no rollout policy; products with one: {known}"
        ))
    })?;
    if !policy.targets.contains_key(target) {
        return Err(CmdError::refused(format!(
            "{product} is not released to {target}; its targets: {}",
            policy
                .targets
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    if policy.targets.keys().all(|named| named == target) {
        return Err(CmdError::refused(format!(
            "{target} is the last target of {product}; a rollout policy needs at least one, so stop releasing \
             the product by release control altogether with `stado release policy remove {product}`"
        )));
    }
    policy.targets.remove(target);
    let store_generation = commit(document, &expected_generation, &mut control).await?;
    Ok(json!({
        "product": product,
        "removed_target": target,
        "release_control_generation": control.generation,
        "store_generation": store_generation,
        "status": "removed",
    }))
}

/// Write `control` back as the next release-control generation (advanced by
/// the registry's declared step, as every release-control writer does),
/// validated against the whole registry and pushed only over
/// `expected_generation`; the store generation the write produced.
pub(super) async fn commit(
    mut document: Value,
    expected_generation: &str,
    control: &mut release_control::ReleaseControl,
) -> Result<String, CmdError> {
    document[release_control::RELEASE_CONTROL_KEY] = serde_json::to_value(&*control)?;
    control.generation = crate::service_resolution::advance_block_generation(
        &mut document,
        release_control::RELEASE_CONTROL_KEY,
    )
    .map_err(|cause| {
        CmdError::click(cause).stating(crate::primitives::failure::FailureCode::Config)
    })?;
    release_control::validate_registry_contract(&document).map_err(CmdError::refused)?;
    crate::cli::registry::push_document_if(&document, expected_generation).await
}
