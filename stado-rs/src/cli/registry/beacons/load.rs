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
    // Names and update times only: a beacon's metadata is never read, so a
    // store that asks for metadata object by object (S3's HEAD) is not asked.
    for blob in store.list_blobs_sized(&prefix).await? {
        let Some(slug) = blob
            .name
            .strip_prefix(&prefix)
            .and_then(|stem| stem.strip_suffix(".json"))
        else {
            continue;
        };
        // A body that is not a JSON object is kept as the beacon with no
        // body and the cause beside it, so the doctor reports a corrupt
        // beacon as corrupt rather than as a host that never published.
        let (body, unreadable) = match store.download_text(&blob.name).await? {
            None => (None, None),
            Some(text) => match serde_json::from_str::<Value>(&text) {
                Ok(Value::Object(map)) => (Some(map), None),
                Ok(other) => (
                    None,
                    Some(format!(
                        "the object is JSON but not an object: it is {}",
                        json_kind(&other)
                    )),
                ),
                Err(error) => (None, Some(format!("the object is not JSON: {error}"))),
            },
        };
        beacons.insert(
            slug.to_string(),
            Beacon {
                path: blob.name.clone(),
                updated: blob.updated,
                body,
                unreadable,
            },
        );
    }
    Ok(beacons)
}

/// The JSON type of a value, in the words the specification uses.
fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// Every beacon in the store that is a JSON object, by slug; one that is not
/// carries no field to read, as [`load_beacons`] leaves its body empty.
pub(crate) async fn beacon_bodies(
    store: &JobStorage,
) -> Result<BTreeMap<String, serde_json::Map<String, Value>>, CmdError> {
    Ok(load_beacons(store)
        .await?
        .into_iter()
        .filter_map(|(slug, beacon)| beacon.body.map(|body| (slug, body)))
        .collect())
}

/// The newest beacon a target resolves to, by the same slug rule
/// `monitor::host_health::load_host_health` resolves forward.
pub(in crate::cli::registry) fn beacon_for_slugs<'a>(
    slugs: &[String],
    beacons: &'a BTreeMap<String, Beacon>,
) -> Option<&'a Beacon> {
    // A beacon whose stamp does not parse orders behind every dated one;
    // the doctor names the unparsable stamp when it reads that beacon.
    let observed = |beacon: &Beacon| match beacon.observed_at() {
        Ok(observed) => observed,
        Err(_unparsable_stamp) => None,
    };
    let mut selected: Option<&Beacon> = None;
    for slug in slugs {
        let Some(candidate) = beacons.get(slug) else {
            continue;
        };
        if selected.is_none_or(|current| observed(candidate) > observed(current)) {
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
