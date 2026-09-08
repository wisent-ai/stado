//! The published job requirement declarations, and which job each service a
//! registry target declares actually runs.

use std::collections::BTreeMap;

use chrono::{DateTime, TimeDelta, Utc};
use serde_json::Value;

use crate::cli::registry::human_age;
use crate::queue::JobStorage;
use crate::targets::ComputeTarget;

/// Object prefix the published job requirement declarations live under, read
/// through the same object API as the beacons and the measurements.
pub(super) const REQUIREMENTS_PREFIX: &str = "job_requirements";

/// The only requirement schema this build understands.
const REQUIREMENTS_SCHEMA: &str = "wisent.trajectory-requirements.v1";

/// How long a published requirement declaration is believed.
///
/// Deliberately not the beacon window: a requirement is republished when the job
/// changes, not on a heartbeat, so judging it in minutes would mark every
/// declaration stale within one and teach operators to skip the row. A week is
/// longer than the fleet goes between Weles releases, so an object older than
/// that is a declaration nobody is republishing — which is the failure this
/// window exists to catch, an object that no longer matches the repository file
/// it is supposed to be a copy of.
fn requirements_stale_after_seconds() -> i64 {
    TimeDelta::weeks(1).num_seconds()
}

/// One declared service and the job it runs.
pub(super) struct RequirementClaim {
    /// Service name as the registry spells it.
    pub(super) unit: String,
    /// The identifier the beacon reports the unit under, so a `missing-plist` row
    /// for the same unit can be recognised as this finding's symptom.
    pub(super) label: String,
    /// Trajectory id the service entry names, e.g. `kimi/login`.
    pub(super) trajectory: String,
}

/// Every published requirement declaration, resolved to what each trajectory
/// needs.
pub(super) struct Declarations {
    /// Trajectory id -> the object that declares it and the capability ids it
    /// names.
    pub(super) needs: BTreeMap<String, (String, Vec<String>)>,
    /// Objects found under the prefix, so a finding can say what was consulted.
    objects: Vec<String>,
    /// Objects this build would not read, and why. A service pointing into one is
    /// reported rather than silently treated as satisfied.
    refused: Vec<String>,
}

impl Declarations {
    /// What was consulted, for a finding that has to explain an absence.
    pub(super) fn consulted(&self) -> String {
        let read = if self.objects.is_empty() {
            format!("no object exists under {REQUIREMENTS_PREFIX}/")
        } else {
            format!("read {}", self.objects.join(", "))
        };
        if self.refused.is_empty() {
            read
        } else {
            format!("{read}; refused {}", self.refused.join("; "))
        }
    }
}

/// Every requirement declaration in the store.
///
/// The declaration is the job author's document, published verbatim
/// (`stado://<namespace>/job_requirements/weles-trajectories.json` carries the
/// bytes of `weles/scripts/trajectories/requirements.json`). The registry names
/// which job a service runs and nothing more, so this is the one place a
/// capability list for a job exists — a copy in the registry would be the second
/// source of truth that `unread-declaration` and this whole command exist to
/// prevent.
pub(super) async fn load_job_requirements(
    store: &JobStorage,
    now: DateTime<Utc>,
) -> Result<Declarations, crate::queue::StorageError> {
    let prefix = format!("{REQUIREMENTS_PREFIX}/");
    let mut declarations = Declarations {
        needs: BTreeMap::new(),
        objects: Vec::new(),
        refused: Vec::new(),
    };
    for blob in store.list_blobs_with_meta(&prefix).await? {
        if !blob.name.ends_with(".json") {
            continue;
        }
        declarations.objects.push(blob.name.clone());
        let Some(body) = store
            .download_text(&blob.name)
            .await?
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        else {
            declarations
                .refused
                .push(format!("{} is not readable JSON", blob.name));
            continue;
        };
        let schema = body
            .get("schema")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if schema != REQUIREMENTS_SCHEMA {
            declarations.refused.push(format!(
                "{} carries schema {schema:?} rather than {REQUIREMENTS_SCHEMA}",
                blob.name
            ));
            continue;
        }
        if let Some(published) = blob.updated {
            let age = now - published;
            if age.num_seconds() > requirements_stale_after_seconds() {
                declarations.refused.push(format!(
                    "{} was published {} ago ({}), past the {}s republication window",
                    blob.name,
                    human_age(age),
                    published.to_rfc3339(),
                    requirements_stale_after_seconds()
                ));
                continue;
            }
        }
        let Some(trajectories) = body.get("trajectories").and_then(Value::as_object) else {
            declarations
                .refused
                .push(format!("{} carries no trajectories map", blob.name));
            continue;
        };
        for (trajectory, value) in trajectories {
            // A non-string entry is dropped rather than guessed at: a garbled
            // requirement must not be able to pass as a satisfied one.
            let capabilities = value
                .as_array()
                .map(|ids| {
                    ids.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            declarations
                .needs
                .insert(trajectory.clone(), (blob.name.clone(), capabilities));
        }
    }
    Ok(declarations)
}

/// Which job each service declared on this target runs.
///
/// The registry's whole role in this join is the identifier: `trajectory` on the
/// service entry, never a capability list. A service that names no trajectory
/// declares no requirement, so every registry written before this field existed
/// stays clean.
pub(super) fn declared_trajectories(target: &ComputeTarget) -> Vec<RequirementClaim> {
    let Some(services) = target.extra.get("services").and_then(Value::as_array) else {
        return Vec::new();
    };
    services
        .iter()
        .filter_map(|entry| {
            let trajectory = entry
                .get("trajectory")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())?;
            let text = |key: &str| {
                entry
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
            };
            let unit = text("name").or_else(|| text("label"));
            Some(RequirementClaim {
                unit: unit.unwrap_or("(unnamed service)").to_string(),
                // The beacon reports a unit under its launchd label or systemd
                // unit, and that is the key the missing-plist row is filed
                // under, so it is resolved here exactly as `declared_units`
                // resolves it.
                label: text("label")
                    .or_else(|| text("unit"))
                    .or(unit)
                    .unwrap_or_default()
                    .to_string(),
                trajectory: trajectory.to_string(),
            })
        })
        .collect()
}
