//! The capability join: what each host was last measured able to do, what
//! each published job declares it needs ([`requirements`]), what stops one
//! from satisfying the other ([`gaps`]), and the doctor rows that follow
//! ([`claims`]).

pub(super) mod claims;
mod gaps;
mod requirements;

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::queue::JobStorage;

/// Object prefix the per-host capability measurements live under, alongside
/// `host_health/` and read through the same object API
/// (`stado://<namespace>/host_capabilities/<registry-target-name>.json`).
const CAPABILITIES_PREFIX: &str = "host_capabilities";

/// The only measurement schema this build understands. A document carrying
/// anything else is reported rather than guessed at: a requirement checked
/// against a shape nobody agreed on is worse than an unchecked one.
const CAPABILITIES_SCHEMA: &str = "wisent.host-capabilities.v1";

/// One measured capability: the answer, and the measurement that produced it.
pub(crate) struct MeasuredCapability {
    pub(crate) value: bool,
    pub(crate) detail: String,
}

/// One `host_capabilities/<target>.json` object.
pub(crate) struct Measurement {
    /// Store-relative object name, so a finding names what to go and read.
    path: String,
    schema: String,
    /// The host's own stamp, falling back to the object mtime the same way
    /// [`Beacon::observed_at`](crate::cli::registry::beacons::beacon::Beacon::observed_at) does.
    pub(crate) measured_at: Option<DateTime<Utc>>,
    pub(crate) capabilities: BTreeMap<String, MeasuredCapability>,
}

/// Every capability measurement in the store, keyed by the registry target name
/// the object is published under.
///
/// One prefix listing plus the bodies it finds, exactly like [`load_beacons`](crate::cli::registry::beacons::load::load_beacons):
/// the two signals are published the same way and are read the same way.
pub(crate) async fn load_capability_measurements(
    store: &JobStorage,
) -> Result<BTreeMap<String, Measurement>, crate::queue::StorageError> {
    let prefix = format!("{CAPABILITIES_PREFIX}/");
    let mut measurements = BTreeMap::new();
    for blob in store.list_blobs_with_meta(&prefix).await? {
        let Some(target) = blob
            .name
            .strip_prefix(&prefix)
            .and_then(|stem| stem.strip_suffix(".json"))
        else {
            continue;
        };
        let Some(body) = store
            .download_text(&blob.name)
            .await?
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        else {
            continue;
        };
        let capabilities = body
            .get("capabilities")
            .and_then(Value::as_object)
            .map(|entries| {
                entries
                    .iter()
                    .map(|(id, entry)| {
                        (
                            id.clone(),
                            MeasuredCapability {
                                value: entry.get("value").and_then(Value::as_bool) == Some(true),
                                detail: entry
                                    .get("detail")
                                    .and_then(Value::as_str)
                                    .unwrap_or("no detail recorded")
                                    .to_string(),
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        measurements.insert(
            target.to_string(),
            Measurement {
                path: blob.name.clone(),
                schema: body
                    .get("schema")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                measured_at: body
                    .get("measured_at")
                    .and_then(Value::as_str)
                    .and_then(|stamp| DateTime::parse_from_rfc3339(stamp).ok())
                    .map(|stamp| stamp.with_timezone(&Utc))
                    .or(blob.updated),
                capabilities,
            },
        );
    }
    Ok(measurements)
}
