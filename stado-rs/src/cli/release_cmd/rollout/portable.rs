//! `stado release policy portable-root PRODUCT [--check] [--json]`: a
//! product whose install root is written under one target's home while its
//! other targets' homes differ (`release_control::foreign_targets`) gets the
//! same path under `{home}`, which each target's release agent expands to its
//! own home. The target whose home the root was written under keeps the very
//! same directory; every other target installs under its own home from the
//! next pass. One verified registry write, read back in the receipt.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::release_control;

use super::ReleasePolicyPortableArgs;

pub(super) async fn portable_root(args: &ReleasePolicyPortableArgs) -> Result<(), CmdError> {
    let report = rewrite(&args.product, args.check).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    let product = &args.product;
    let portable = crate::cli::reporting::field(&report["portable_root"]);
    match report["status"].as_str() {
        Some("already_portable") => println!("{product} already installs at {portable}, under each target's own home"),
        Some("would_rewrite") => println!(
            "{product} would install at {portable} instead of {}: {}",
            crate::cli::reporting::field(&report["install_root"]),
            outside(&report)
        ),
        _ => println!(
            "{product} installs at {portable} now (release-control generation {}); {} installs under its own home \
             from its release agent's next pass",
            crate::cli::reporting::field(&report["release_control_generation"]),
            outside(&report)
        ),
    }
    Ok(())
}

/// The targets the old root sent outside their homes, as the text names them.
fn outside(report: &Value) -> String {
    report["outside_home"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|row| {
            format!(
                "{} (home {})",
                crate::cli::reporting::field(&row["host"]),
                crate::cli::reporting::field(&row["home"])
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Rewrite `product`'s install root under `{home}` (unless `check`), refusing
/// an unknown product and a root under no target's home, which has no `{home}`
/// form; a root that already names `{home}` is reported, not rewritten.
async fn rewrite(product: &str, check: bool) -> Result<Value, CmdError> {
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
    let install_root = policy.install_root.clone();
    if install_root.contains(release_control::HOME_PLACEHOLDER) {
        return Ok(json!({
            "product": product,
            "install_root": install_root,
            "portable_root": install_root,
            "status": "already_portable",
        }));
    }
    let foreign = release_control::foreign_targets(policy);
    let Some(first) = foreign.first() else {
        return Err(CmdError::refused(format!(
            "{product} installs at {install_root}, which lies under no target's home that another target does not \
             share, so it has no {} form to write; edit it with stado release policy show {product} and stado release \
             policy apply",
            release_control::HOME_PLACEHOLDER
        )));
    };
    let portable_root = first.portable_root.clone();
    let mut report = json!({
        "product": product,
        "install_root": install_root,
        "portable_root": portable_root,
        "written_under": first.owner,
        "outside_home": foreign.iter().map(|target| json!({"host": target.host, "home": target.home})).collect::<Vec<_>>(),
        "status": "would_rewrite",
    });
    if check {
        return Ok(report);
    }
    policy.install_root = portable_root;
    let store_generation =
        super::policy::commit(document, &expected_generation, &mut control).await?;
    report["status"] = json!("rewritten");
    report["release_control_generation"] = json!(control.generation);
    report["store_generation"] = json!(store_generation);
    Ok(report)
}
