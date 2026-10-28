//! A release-controlled tree installed directly where the catalog program of
//! its service runs, for a release control that installs it elsewhere.

use super::super::template::ServicePlan;
use crate::cli::CmdError;
use crate::targets::ComputeTarget;

/// Install the desired release of `product` on `target` where the catalog
/// program of `service` runs, `$HOME/.stado/services/<tree>/current`, through
/// the same verified fetch the release agent uses and the archive installer
/// `stado service release` uses; what it installed.
pub(super) async fn install_tree(
    product: &str,
    tree: &str,
    target: &ComputeTarget,
    service: &ServicePlan,
) -> Result<String, CmdError> {
    let document = crate::cli::registry::fetch_document().await?;
    let control = crate::release_control::control(&document)
        .map_err(CmdError::declaration)?
        .ok_or_else(|| CmdError::declaration("registry.release_control is not configured"))?;
    let policy = control.products.get(product).ok_or_else(|| {
        CmdError::missing(format!(
            "registry.release_control declares no product {product:?}"
        ))
    })?;
    let target_policy = policy.targets.get(&target.name).ok_or_else(|| {
        CmdError::missing(format!(
            "product {product:?} has no release target {:?}",
            target.name
        ))
    })?;
    let desired = policy
        .desired
        .as_ref()
        .ok_or_else(|| CmdError::missing(format!("product {product:?} has no desired release")))?;
    let artifact = desired
        .artifacts
        .get(&target_policy.platform)
        .ok_or_else(|| {
            CmdError::missing(format!(
                "{product} {} has no {} artifact",
                desired.version, target_policy.platform
            ))
        })?;
    let (manifest, archive, _) = crate::release_agent::fetch_candidate(
        &control,
        product,
        desired,
        artifact,
        policy,
        target_policy,
    )
    .await?;
    let entry = crate::deploy::service_catalog::lookup(&service.catalog_name)
        .map_err(CmdError::declaration)?
        .ok_or_else(|| {
            CmdError::missing(format!(
                "the catalog no longer declares {}",
                service.catalog_name
            ))
        })?;
    let member = entry
        .program
        .split_once("/current/")
        .map(|(_, member)| member)
        .ok_or_else(|| {
            CmdError::declaration(format!(
                "the catalog program {} of {tree} is not under current/",
                entry.program
            ))
        })?;
    let required = crate::deploy::service_catalog::resolve_word(
        member,
        "",
        &target.release_platform,
        &target.name,
    );
    let staged = crate::cli::service::lifecycle::release::gate::stage_service_release_archive(
        product,
        &manifest.version,
        &target_policy.platform,
        &archive,
    )?;
    let path = staged.to_str().ok_or_else(|| {
        CmdError::declaration(format!(
            "the staged archive path {} is not UTF-8",
            staged.display()
        ))
    })?;
    let runner = crate::deploy::production_runner();
    let (installed, _) = crate::cli::service::lifecycle::release::install_from_archive(
        target, tree, path, &required, &runner,
    )
    .await?;
    Ok(format!(
        "{product} {} installed at {} (version {}) for the catalog program {required}",
        manifest.version, installed.program_path, installed.version
    ))
}

/// Write `root` (a `{home}`-relative install root) as product `product`'s
/// install root in release control, in one compare-and-swapped commit that
/// counts as one release-control generation; a policy that says so already
/// is left alone. An absolute root under one host's home sends every other
/// target's agent there; the `{home}` form is each target's own home.
pub(super) async fn rewrite_install_root(product: &str, root: &str) -> Result<String, CmdError> {
    let product = product.to_string();
    let root = root.to_string();
    crate::cli::registry::commit_document(move |current| {
        let mut next = current.clone();
        let control = next
            .get_mut("release_control")
            .and_then(serde_json::Value::as_object_mut)
            .ok_or_else(|| CmdError::declaration("registry.release_control is not an object"))?;
        let generation = control
            .get("generation")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| {
                CmdError::declaration("registry.release_control.generation is not an integer")
            })?;
        let policy = control
            .get_mut("products")
            .and_then(|products| products.get_mut(&product))
            .and_then(serde_json::Value::as_object_mut)
            .ok_or_else(|| {
                CmdError::missing(format!("release control lost its product {product}"))
            })?;
        if policy
            .get("install_root")
            .and_then(serde_json::Value::as_str)
            == Some(root.as_str())
        {
            return Ok(current.clone());
        }
        policy.insert(
            "install_root".to_string(),
            serde_json::Value::from(root.clone()),
        );
        let mut next_generation = generation;
        crate::primitives::tally::Tally::tally(&mut next_generation);
        control.insert(
            "generation".to_string(),
            serde_json::Value::from(next_generation),
        );
        Ok(next)
    })
    .await
}
