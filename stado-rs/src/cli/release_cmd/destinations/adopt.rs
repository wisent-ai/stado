//! Adopt existing delivery authority before a host-independent recipe replaces it.

use crate::cli::{registry, CmdError};
use crate::release_pipeline::{
    self, destinations, Delivery, DeliveryTarget, ProductManifest, ReleaseCatalogEntry,
    ReleasePipelineManifest,
};
use serde_json::Value;
use std::collections::BTreeSet;

fn pinned(manifest: &ReleasePipelineManifest) -> BTreeSet<String> {
    manifest
        .deliveries
        .iter()
        .filter_map(|delivery| match &delivery.target {
            DeliveryTarget::Host(host) if !host.is_empty() => Some(host.clone()),
            _ => None,
        })
        .collect()
}

pub(super) async fn from_catalog(product: &str) -> Result<(Vec<String>, String), CmdError> {
    destinations::validate_product(product).map_err(CmdError::click)?;
    let bytes =
        crate::cli::storage::fetch_object(&crate::cli::release_catalog::catalog_uri(product))
            .await?;
    let entry: ReleaseCatalogEntry = serde_json::from_slice(&bytes)?;
    release_pipeline::validate_catalog_entry(&entry).map_err(CmdError::click)?;
    if entry.product != product {
        return Err(CmdError::refused(
            "catalog product disagrees with the requested coordinate",
        ));
    }
    let ProductManifest::Release(manifest) = &entry.manifest else {
        return Err(CmdError::refused(
            "catalog product does not declare releases",
        ));
    };
    let targets: Vec<String> = pinned(manifest).into_iter().collect();
    if targets.is_empty() {
        return Err(CmdError::refused("catalog has no explicit delivery hosts to adopt; declare the target set with release destinations set"));
    }
    let generation = registry::commit_document(|document| {
        if destinations::declarations(document).map_err(CmdError::click)?.is_some_and(|entries| entries.contains_key(product)) {
            let current: BTreeSet<String> = destinations::read(document, product).map_err(CmdError::click)?
                .into_iter().map(|destination| destination.target).collect();
            if current != targets.iter().cloned().collect() {
                return Err(CmdError::refused("adoption would replace an existing destination declaration; use release destinations set explicitly"));
            }
            return Ok(document.clone());
        }
        super::state::put(document, product, &targets)
    }).await?;
    Ok((targets, generation))
}

fn same_operation(left: &Delivery, right: &Delivery) -> bool {
    left.platform == right.platform
        && left.argv == right.argv
        && left.required == right.required
        && left.secret_env == right.secret_env
        && left.after.is_empty()
        && right.after.is_empty()
}

fn preserve_operations(
    document: &Value,
    old: &ReleasePipelineManifest,
    new: &ReleasePipelineManifest,
) -> Result<(), CmdError> {
    let targets = destinations::read(document, &new.product).map_err(CmdError::click)?;
    for delivery in &new.deliveries {
        if !matches!(delivery.target, DeliveryTarget::Registry(_)) {
            continue;
        }
        let actual: BTreeSet<&str> = targets
            .iter()
            .filter(|target| target.platform == new.platforms[&delivery.platform].runner_platform)
            .map(|target| target.target.as_str())
            .collect();
        let previous: BTreeSet<&str> = old
            .deliveries
            .iter()
            .filter(|prior| same_operation(prior, delivery))
            .filter_map(|prior| match &prior.target {
                DeliveryTarget::Host(host) if !host.is_empty() => Some(host.as_str()),
                _ => None,
            })
            .collect();
        if previous.is_empty() || actual != previous {
            return Err(CmdError::refused(format!(
                "cannot adopt destinations for {} without changing delivery {}; declare destinations explicitly before publishing this recipe",
                new.product, delivery.name
            )));
        }
    }
    for prior in &old.deliveries {
        if !new.deliveries.iter().any(|delivery| {
            delivery == prior
                || matches!(delivery.target, DeliveryTarget::Registry(_))
                    && same_operation(prior, delivery)
        }) {
            return Err(CmdError::refused(format!(
                "destination adoption would drop catalog delivery {}",
                prior.name
            )));
        }
    }
    Ok(())
}

pub(crate) async fn migrate(
    previous: Option<&[u8]>,
    proposed: &ReleaseCatalogEntry,
) -> Result<(), CmdError> {
    let ProductManifest::Release(new) = &proposed.manifest else {
        return Ok(());
    };
    if !new
        .deliveries
        .iter()
        .any(|delivery| matches!(delivery.target, DeliveryTarget::Registry(_)))
    {
        return Ok(());
    }
    let (document, _) = registry::fetch_versioned_document().await?;
    if destinations::declarations(&document)
        .map_err(CmdError::click)?
        .is_some_and(|entries| entries.contains_key(&new.product))
    {
        destinations::read(&document, &new.product).map_err(CmdError::click)?;
        return Ok(());
    }
    let bytes = previous.ok_or_else(|| CmdError::refused(format!(
        "{} has no delivery destination declaration or earlier catalog authority; use stado release destinations set {} --target <HOST>",
        new.product, new.product
    )))?;
    let previous: ReleaseCatalogEntry = serde_json::from_slice(bytes)?;
    release_pipeline::validate_catalog_entry(&previous).map_err(CmdError::click)?;
    if previous.product != new.product {
        return Err(CmdError::refused(
            "prior catalog product disagrees with the destination product",
        ));
    }
    let ProductManifest::Release(old) = &previous.manifest else {
        return Err(CmdError::refused(
            "prior catalog entry has no release deliveries to adopt",
        ));
    };
    let targets: BTreeSet<String> = old
        .deliveries
        .iter()
        .filter(|prior| {
            new.deliveries.iter().any(|delivery| {
                matches!(delivery.target, DeliveryTarget::Registry(_))
                    && same_operation(prior, delivery)
            })
        })
        .filter_map(|prior| match &prior.target {
            DeliveryTarget::Host(host) if !host.is_empty() => Some(host.clone()),
            _ => None,
        })
        .collect();
    let targets: Vec<String> = targets.into_iter().collect();
    registry::commit_document(|current| {
        if destinations::declarations(current)
            .map_err(CmdError::click)?
            .is_some_and(|entries| entries.contains_key(&new.product))
        {
            destinations::read(current, &new.product).map_err(CmdError::click)?;
            return Ok(current.clone());
        }
        let next = super::state::put(current, &new.product, &targets)?;
        preserve_operations(&next, old, new)?;
        Ok(next)
    })
    .await?;
    Ok(())
}
