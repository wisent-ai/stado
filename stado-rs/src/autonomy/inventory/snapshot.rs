//! The report: fan the readers out, fold the adoptions in, seal the result.
//!
//! [`collect`] runs the four readers, stamps every adopted resource with its
//! owner and policy reference, rewrites the dependency edges into resource
//! ids and hands the finished [`InventorySnapshot`] to [`reseal`], which
//! recomputes the canonical digest the snapshot is addressed by.
//! [`collect_and_publish`] is that same read followed by the write to the
//! object store.

use std::collections::{BTreeMap, BTreeSet};

use chrono::Utc;

use crate::autonomy::model::{
    InventorySnapshot, InventorySource, Ownership, ResourceGraph, ResourceRecord, SourceState,
    SCHEMA_VERSION,
};
use crate::capabilities::ProviderId;
use crate::cli::resources::model::canonical_json_bytes;
use crate::queue::{JobStorage, StorageError};

use super::sources::{collect_aws, collect_azure, collect_gcp, collect_local};
use super::values::sha256_hex;

pub async fn collect(store: &JobStorage) -> Result<InventorySnapshot, StorageError> {
    let observed_at = Utc::now();
    let configured: BTreeSet<ProviderId> = crate::config::wc_providers()
        .iter()
        .filter_map(|name| crate::capabilities::provider(name))
        .collect();
    let (gcp, aws, azure) = tokio::join!(
        async {
            if configured.contains(&ProviderId::Gcp) {
                Some(collect_gcp(observed_at).await)
            } else {
                None
            }
        },
        async {
            if configured.contains(&ProviderId::Aws) {
                Some(collect_aws(observed_at).await)
            } else {
                None
            }
        },
        async {
            if configured.contains(&ProviderId::Azure) {
                Some(collect_azure(observed_at).await)
            } else {
                None
            }
        },
    );
    let mut sources: Vec<InventorySource> = [gcp, aws, azure].into_iter().flatten().collect();
    sources.push(collect_local(store, observed_at).await?);
    let adoptions = super::storage::list_adoptions(store).await?;
    let adopted: BTreeMap<&str, _> = adoptions
        .iter()
        .map(|record| (record.resource_id.as_str(), record))
        .collect();
    for source in &mut sources {
        for resource in &mut source.resources {
            if let Some(record) = adopted.get(resource.resource_id.as_str()) {
                resource.ownership = Ownership::Adopted;
                resource.owner = Some(record.owner.clone());
                resource.policy_ref = Some(record.policy_ref.clone());
            }
        }
    }
    let mut resources: Vec<ResourceRecord> = sources
        .iter()
        .flat_map(|source| source.resources.iter().cloned())
        .collect();
    resolve_dependencies(&mut resources);
    resources.sort_by(|left, right| left.resource_id.cmp(&right.resource_id));
    let graph = ResourceGraph::from_resources(&resources);
    let complete = sources
        .iter()
        .all(|source| source.state == SourceState::Complete);
    let mut snapshot = InventorySnapshot {
        schema_version: SCHEMA_VERSION,
        snapshot_id: String::new(),
        created_at: observed_at.to_rfc3339(),
        complete,
        sources,
        resources,
        graph,
    };
    reseal(&mut snapshot)?;
    Ok(snapshot)
}

pub fn reseal(snapshot: &mut InventorySnapshot) -> Result<(), StorageError> {
    snapshot.snapshot_id.clear();
    let digest = sha256_hex(&canonical_json_bytes(snapshot).map_err(|error| {
        StorageError::Other(format!("inventory canonicalization failed: {error}"))
    })?);
    snapshot.snapshot_id = digest;
    Ok(())
}

pub async fn collect_and_publish(store: &JobStorage) -> Result<InventorySnapshot, StorageError> {
    let snapshot = collect(store).await?;
    super::storage::publish_inventory(store, &snapshot).await?;
    Ok(snapshot)
}

fn resolve_dependencies(resources: &mut [ResourceRecord]) {
    let mut aliases = BTreeMap::new();
    for resource in resources.iter() {
        aliases.insert(
            resource.native_reference.clone(),
            resource.resource_id.clone(),
        );
        aliases.insert(resource.name.clone(), resource.resource_id.clone());
        if let Some(tail) = resource.native_reference.rsplit('/').next() {
            aliases
                .entry(tail.to_string())
                .or_insert_with(|| resource.resource_id.clone());
        }
    }
    for resource in resources {
        resource.dependencies = resource
            .dependencies
            .iter()
            .filter_map(|reference| aliases.get(reference).cloned())
            .filter(|dependency| dependency != &resource.resource_id)
            .collect();
    }
}
