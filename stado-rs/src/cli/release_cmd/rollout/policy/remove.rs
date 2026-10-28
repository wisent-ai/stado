//! `stado release policy remove PRODUCT`: a product release control no
//! longer rolls out anywhere.

use serde_json::json;

use super::super::ReleasePolicyRemoveArgs;
use super::commit;
use crate::cli::CmdError;
use crate::release_control;

/// `stado release policy remove PRODUCT`: the product is no longer rolled out
/// by release control anywhere. One verified registry write removes its
/// policy, and each former target's release agent then retires the proxy and
/// release processes it ran for the product. The validator refuses the write
/// while anything in the registry still names the product as
/// release-controlled.
pub(super) async fn remove_policy(args: &ReleasePolicyRemoveArgs) -> Result<(), CmdError> {
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
    let store_generation = commit(document, &expected_generation, &mut control).await?;
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
