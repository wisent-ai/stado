//! Resolve once per run. Resume and redelivery consume the retained placement,
//! never a newly selected host set from a changed registry.

use crate::cli::release_submit::run::source::{queue_immutable, run_path};
use crate::cli::{registry, CmdError};
use crate::queue::storage::JobStorage;
use crate::release_pipeline::{
    destinations, Delivery, DeliveryTarget, ReleasePipelineManifest, ReleaseRun,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Placement {
    schema_version: u32,
    run_id: String,
    source_sha256: String,
    manifest_sha256: String,
    registry_generation: String,
    destinations: Vec<destinations::Destination>,
}

fn path(run: &ReleaseRun) -> String {
    run_path(&run.product, &run.run_id, "deliveries/placement.json")
}

fn uses_registry(manifest: &ReleasePipelineManifest) -> bool {
    manifest
        .deliveries
        .iter()
        .any(|delivery| matches!(delivery.target, DeliveryTarget::Registry(_)))
}

async fn read(store: &JobStorage, run: &ReleaseRun) -> Result<Option<Placement>, CmdError> {
    let Some(bytes) = store.read_bytes(&path(run)).await? else {
        return Ok(None);
    };
    let plan: Placement = serde_json::from_slice(&bytes)?;
    if plan.schema_version != 1
        || plan.run_id != run.run_id
        || plan.source_sha256 != run.source_sha256
        || plan.manifest_sha256 != run.manifest_sha256
        || plan.registry_generation.is_empty()
    {
        return Err(
            CmdError::click("release delivery placement does not match its immutable run")
                .stating(crate::primitives::failure::FailureCode::InfraDown),
        );
    }
    Ok(Some(plan))
}

pub(in crate::cli::release_submit) async fn prepare(
    store: &JobStorage,
    run: &ReleaseRun,
    manifest: &ReleasePipelineManifest,
) -> Result<Vec<Delivery>, CmdError> {
    if !uses_registry(manifest) {
        return Ok(manifest.deliveries.clone());
    }
    if let Some(plan) = read(store, run).await? {
        return expand(manifest, &plan.destinations);
    }
    if !run.deliveries.is_empty() {
        return Err(CmdError::refused("release has delivery attempts but its immutable placement is missing; refusing to choose replacement hosts"));
    }
    let (document, generation) = registry::fetch_versioned_document().await?;
    let plan = Placement {
        schema_version: 1,
        run_id: run.run_id.clone(),
        source_sha256: run.source_sha256.clone(),
        manifest_sha256: run.manifest_sha256.clone(),
        registry_generation: generation,
        destinations: destinations::read(&document, &manifest.product)
            .map_err(CmdError::declaration)?,
    };
    let deliveries = expand(manifest, &plan.destinations)?;
    queue_immutable(&path(run), &serde_json::to_vec(&plan)?).await?;
    Ok(deliveries)
}

pub(in crate::cli::release_submit) async fn recorded(
    store: &JobStorage,
    run: &ReleaseRun,
    manifest: &ReleasePipelineManifest,
) -> Result<Vec<Delivery>, CmdError> {
    if !uses_registry(manifest) {
        return Ok(manifest.deliveries.clone());
    }
    let plan = read(store, run).await?.ok_or_else(|| {
        CmdError::missing(
            "release delivery placement is missing; redelivery cannot choose destinations from the current registry",
        )
    })?;
    expand(manifest, &plan.destinations)
}

fn expand(
    manifest: &ReleasePipelineManifest,
    destinations: &[destinations::Destination],
) -> Result<Vec<Delivery>, CmdError> {
    let mut hosts = BTreeSet::new();
    for destination in destinations {
        destination.validate().map_err(CmdError::declaration)?;
        if !hosts.insert(destination.target.as_str()) {
            return Err(CmdError::click(format!(
                "delivery placement repeats target {}",
                destination.target
            ))
            .stating(crate::primitives::failure::FailureCode::Config));
        }
    }
    let mut used = BTreeSet::new();
    let mut deliveries: Vec<Delivery> = Vec::new();
    let mut ranges: BTreeMap<&str, Range<usize>> = BTreeMap::new();
    for declared in &manifest.deliveries {
        let start = deliveries.len();
        let mut push = |name, target| -> Result<(), CmdError> {
            let mut after = Vec::new();
            for dependency in &declared.after {
                let range = ranges.get(dependency.as_str()).ok_or_else(|| {
                    CmdError::click(format!(
                        "delivery {} names an unresolved predecessor {dependency}",
                        declared.name
                    ))
                    .stating(crate::primitives::failure::FailureCode::Config)
                })?;
                after.extend(
                    deliveries[range.clone()]
                        .iter()
                        .map(|prior| prior.name.clone()),
                );
            }
            deliveries.push(Delivery {
                name,
                platform: declared.platform.clone(),
                argv: declared.argv.clone(),
                required: declared.required,
                secret_env: declared.secret_env.clone(),
                target,
                after,
            });
            Ok(())
        };
        match &declared.target {
            DeliveryTarget::Host(_) => push(declared.name.clone(), declared.target.clone())?,
            DeliveryTarget::Registry(_) => {
                for destination in destinations.iter().filter(|destination| {
                    destination.platform == manifest.platforms[&declared.platform].runner_platform
                }) {
                    used.insert(destination.target.as_str());
                    push(
                        format!("{}--{}", declared.name, destination.target),
                        DeliveryTarget::Host(destination.target.clone()),
                    )?;
                }
            }
        }
        if deliveries.len() == start {
            return Err(CmdError::refused(format!(
                "delivery {} has no declared destinations for platform {}; no builder fallback is allowed",
                declared.name, manifest.platforms[&declared.platform].runner_platform
            )));
        }
        ranges.insert(&declared.name, start..deliveries.len());
    }
    if used != hosts {
        return Err(CmdError::refused(format!(
            "declared release destinations have no matching delivery platform: {}",
            hosts
                .difference(&used)
                .copied()
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    let mut names: Vec<&str> = deliveries
        .iter()
        .map(|delivery| delivery.name.as_str())
        .collect();
    names.sort_unstable();
    if let Some(pair) = names.windows(2).find(|pair| pair[0] == pair[1]) {
        return Err(CmdError::refused(format!(
            "resolved delivery name is ambiguous: {}",
            pair[0]
        )));
    }
    Ok(deliveries)
}
