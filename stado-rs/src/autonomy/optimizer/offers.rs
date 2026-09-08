//! The offer read: every unit of capacity the pass is allowed to place on.
//!
//! [`collect_offers`] folds the consumer capacity the agents report and the
//! provider quota for capacity that does not exist yet into one list of
//! [`CapacityOffer`]s, dropping whatever the policy does not allow. The
//! shape and region helpers below are what an offer is filled in from, and
//! [`offer_regions`] is the set of regions an offer could actually land in.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::Value;

use crate::autonomy::policy::AutonomyPolicy;
use crate::capabilities::ProviderId;
use crate::providers::Provider;
use crate::queue::{capacity, JobStorage, StorageError};

use super::types::CapacityOffer;

fn payload_text<'a>(payload: &'a Value, key: &str) -> Option<&'a str> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .or_else(|| payload.get("diag")?.get(key)?.as_str())
}

pub(super) async fn collect_offers(
    store: &JobStorage,
    cloud_providers: &[(String, Arc<dyn Provider>)],
    policy: &AutonomyPolicy,
) -> Result<(Vec<CapacityOffer>, BTreeMap<String, String>), StorageError> {
    let mut offers = Vec::new();
    let mut errors = BTreeMap::new();
    let consumers = capacity::read_consumer_capacity(store).await?;
    for (consumer_id, payload) in consumers {
        let Some(kind) = payload.get("kind").and_then(Value::as_str) else {
            continue;
        };
        let Some(provider) = crate::capabilities::provider(kind) else {
            continue;
        };
        if !policy.placement.allowed_providers.contains(&provider) {
            continue;
        }
        if payload.get("accepting_jobs").and_then(Value::as_bool) != Some(true) {
            continue;
        }
        let free_vram = payload
            .get("free_vram_gb")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let available = payload
            .get("available_accelerators")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        if available.is_empty() && free_vram > i64::default() {
            let accelerator = payload_text(&payload, "gpu_type").unwrap_or("");
            offers.push(CapacityOffer {
                target_id: consumer_id,
                provider,
                region: payload_text(&payload, "region")
                    .map(str::to_string)
                    .or_else(|| default_region(provider)),
                accelerator_type: accelerator.to_string(),
                machine_type: payload_text(&payload, "machine_type")
                    .map(str::to_string)
                    .or_else(|| {
                        sizing_for_accelerator(provider.as_str(), accelerator)
                            .map(|(_, machine)| machine)
                    })
                    .unwrap_or_default(),
                free_vram_gb: free_vram,
                available_instances: 1,
                existing: true,
            });
            continue;
        }
        for (accelerator, count) in available {
            let count = count.as_i64().unwrap_or_default();
            if count <= i64::default() {
                continue;
            }
            offers.push(CapacityOffer {
                target_id: consumer_id.clone(),
                provider,
                region: payload_text(&payload, "region")
                    .map(str::to_string)
                    .or_else(|| default_region(provider)),
                accelerator_type: accelerator.clone(),
                machine_type: payload_text(&payload, "machine_type")
                    .map(str::to_string)
                    .or_else(|| {
                        sizing_for_accelerator(provider.as_str(), &accelerator)
                            .map(|(_, machine)| machine)
                    })
                    .unwrap_or_default(),
                free_vram_gb: free_vram,
                available_instances: count,
                existing: true,
            });
        }
    }
    for (name, provider) in cloud_providers {
        let Some(provider_id) = crate::capabilities::provider(name) else {
            continue;
        };
        if !policy.placement.allowed_providers.contains(&provider_id) {
            continue;
        }
        match crate::scheduler::quota::get_available_instances(store, provider.as_ref(), name).await
        {
            Ok(available) => {
                for (accelerator, count) in available {
                    if count <= i64::default() {
                        continue;
                    }
                    let Some((vram, machine)) = sizing_for_accelerator(name, &accelerator) else {
                        continue;
                    };
                    offers.push(CapacityOffer {
                        target_id: format!("{name}:new:{machine}"),
                        provider: provider_id,
                        region: default_region(provider_id),
                        accelerator_type: accelerator,
                        machine_type: machine,
                        free_vram_gb: vram,
                        available_instances: count,
                        existing: false,
                    });
                }
            }
            Err(error) => {
                errors.insert(name.clone(), error.to_string());
            }
        }
    }
    Ok((offers, errors))
}

fn sizing_for_accelerator(provider: &str, accelerator: &str) -> Option<(i64, String)> {
    crate::catalog::GPU_SIZING
        .get(provider)?
        .iter()
        .find(|(_, (_, candidate))| *candidate == accelerator)
        .map(|(vram, (machine, _))| (*vram, (*machine).to_string()))
}

fn default_region(provider: ProviderId) -> Option<String> {
    match provider {
        ProviderId::Gcp => crate::config::zone_rotation()
            .first()
            .and_then(|zone| zone.rsplit_once('-').map(|(region, _)| region.to_string())),
        ProviderId::Azure => crate::config::azure_locations().first().cloned(),
        ProviderId::Aws => Some(crate::config::aws_region().to_string()),
        _ => None,
    }
}

pub(super) fn offer_regions(offer: &CapacityOffer) -> BTreeSet<String> {
    let mut regions = if offer.existing {
        offer.region.iter().cloned().collect()
    } else {
        match offer.provider {
            ProviderId::Gcp => {
                let zones = crate::config::machine_type_zones()
                    .get(&offer.machine_type)
                    .map(Vec::as_slice)
                    .unwrap_or_else(|| crate::config::zone_rotation());
                zones
                    .iter()
                    .filter_map(|zone| zone.rsplit_once('-').map(|(region, _)| region.to_string()))
                    .collect()
            }
            ProviderId::Azure => crate::config::azure_locations().iter().cloned().collect(),
            ProviderId::Aws => BTreeSet::from([crate::config::aws_region().to_string()]),
            _ => BTreeSet::new(),
        }
    };
    if regions.is_empty() {
        regions.extend(offer.region.iter().cloned());
    }
    regions
}
