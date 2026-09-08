//! Every beacon in the store, and the slug rule that resolves a registry
//! target to the one that proves it is alive.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::cli::registry::beacons::beacon::Beacon;
use crate::cli::CmdError;
use crate::monitor::host_health;
use crate::queue::JobStorage;
use crate::targets::ComputeTarget;

/// Every beacon in the store, keyed by slug — the `<slug>.json` stem that
/// `monitor::host_health::beacon_slugs` resolves targets to.
pub(in crate::cli::registry) async fn load_beacons(
    store: &JobStorage,
) -> Result<BTreeMap<String, Beacon>, CmdError> {
    let prefix = format!("{}/", host_health::HEALTH_PREFIX);
    let mut beacons = BTreeMap::new();
    for blob in store.list_blobs_with_meta(&prefix).await? {
        let Some(slug) = blob
            .name
            .strip_prefix(&prefix)
            .and_then(|stem| stem.strip_suffix(".json"))
        else {
            continue;
        };
        let body = store
            .download_text(&blob.name)
            .await?
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|value| match value {
                Value::Object(map) => Some(map),
                _ => None,
            });
        beacons.insert(
            slug.to_string(),
            Beacon {
                path: blob.name.clone(),
                updated: blob.updated,
                body,
            },
        );
    }
    Ok(beacons)
}

/// The newest beacon a target resolves to, by the same slug rule
/// `monitor::host_health::load_host_health` resolves forward.
pub(in crate::cli::registry) fn beacon_for_slugs<'a>(
    slugs: &[String],
    beacons: &'a BTreeMap<String, Beacon>,
) -> Option<&'a Beacon> {
    let mut selected: Option<&Beacon> = None;
    for slug in slugs {
        let Some(candidate) = beacons.get(slug) else {
            continue;
        };
        if selected.is_none_or(|current| candidate.observed_at() > current.observed_at()) {
            selected = Some(candidate);
        }
    }
    selected
}

pub(super) fn beacon_for<'a>(
    target: &ComputeTarget,
    beacons: &'a BTreeMap<String, Beacon>,
) -> Option<&'a Beacon> {
    let slugs = host_health::beacon_slugs(target, &target.name);
    beacon_for_slugs(&slugs, beacons)
}
