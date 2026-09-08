//! `stado registry beacon-age` — every registry host paired with the beacon
//! that proves it is alive, worst first.

use chrono::{DateTime, TimeDelta, Utc};
use serde_json::{json, Value};

use crate::cli::registry::beacons::beacon::Beacon;
use crate::cli::registry::beacons::load::{beacon_for, load_beacons};
use crate::cli::registry::echo_json;
use crate::cli::registry::write::document::fetch_versioned_document;
use crate::cli::{table, CmdError};
use crate::queue::JobStorage;
use crate::targets;

/// Sort rank, worst first. Derived `Ord` follows declaration order, so a
/// host that never reported outranks a stale one and a target that is not
/// supposed to have a beacon sinks to the bottom.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum BeaconRank {
    /// kind=local with no beacon object at all.
    Missing,
    /// Has a beacon; ordered oldest-first within the rank.
    Reported,
    /// Not a machine (kind=gcp / kind=vast): no beacon is expected.
    NotExpected,
}

impl BeaconRank {
    fn label(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Reported => "reported",
            Self::NotExpected => "not-applicable",
        }
    }
}

/// One registry host paired with the beacon that proves it is alive.
struct BeaconRow {
    name: String,
    kind: String,
    rank: BeaconRank,
    observed: Option<DateTime<Utc>>,
    reported_at: Option<String>,
    path: Option<String>,
}

/// Largest whole unit of an age, e.g. `5d` — the "has not reported in
/// days" signal at a glance. chrono performs every unit conversion, so no
/// seconds-per-day arithmetic is hand-rolled here.
///
/// Shared with `cli::host::ping`, which grades the same beacon for a
/// single host: one spelling of an age across both commands.
pub(crate) fn human_age(age: TimeDelta) -> String {
    for (amount, suffix) in [
        (age.num_days(), "d"),
        (age.num_hours(), "h"),
        (age.num_minutes(), "m"),
    ] {
        if amount.is_positive() {
            return format!("{amount}{suffix}");
        }
    }
    format!("{}s", age.num_seconds().max(i64::default()))
}

/// `stado registry beacon-age [--json]` — every registry host and its last
/// beacon, worst first.
///
/// Lists hosts with no beacon at all: a machine that silently stopped
/// reporting is exactly what this table exists to surface, and a row that
/// is absent surfaces nothing.
pub async fn beacon_age(as_json: bool) -> Result<(), CmdError> {
    let (document, _) = fetch_versioned_document().await?;
    let registry = targets::load_registry_from_value(&document).map_err(|error| {
        CmdError::click(format!(
            "invalid registry document at {}: {error}",
            targets::registry_location()
        ))
    })?;
    let store = JobStorage::for_primary_reads().await?;
    let beacons = load_beacons(&store).await?;
    let now = Utc::now();

    let mut rows: Vec<BeaconRow> = registry
        .targets
        .iter()
        .map(|target| {
            let beacon = beacon_for(target, &beacons);
            let rank = if beacon.is_some() {
                BeaconRank::Reported
            } else if target.is_provider(crate::capabilities::ProviderId::Local) {
                BeaconRank::Missing
            } else {
                BeaconRank::NotExpected
            };
            BeaconRow {
                name: target.name.clone(),
                kind: target.kind.clone(),
                rank,
                observed: beacon.and_then(Beacon::observed_at),
                reported_at: beacon.and_then(Beacon::reported_at).map(str::to_string),
                path: beacon.map(|beacon| beacon.path.clone()),
            }
        })
        .collect();
    // (rank, observed) ascending: never-reported first, then the oldest
    // beacon, with the not-applicable rows last.
    rows.sort_by_key(|row| (row.rank, row.observed));

    let age_of = |row: &BeaconRow| row.observed.map(|observed| now - observed);
    if as_json {
        let hosts: Vec<Value> = rows
            .iter()
            .map(|row| {
                json!({
                    "host": row.name,
                    "kind": row.kind,
                    "status": row.rank.label(),
                    "beacon": row.path,
                    "observed_at": row.observed.map(|ts| ts.to_rfc3339()),
                    "reported_at": row.reported_at,
                    "age_seconds": age_of(row).map(|age| age.num_seconds()),
                })
            })
            .collect();
        echo_json(&json!({"registry": targets::registry_location(), "hosts": hosts}));
        return Ok(());
    }

    let table_rows: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            let age = match (row.rank, age_of(row)) {
                (BeaconRank::Missing, _) => "never".to_string(),
                (BeaconRank::NotExpected, None) => "-".to_string(),
                (_, Some(age)) => human_age(age),
                (_, None) => "unknown".to_string(),
            };
            vec![
                row.name.clone(),
                row.kind.clone(),
                age,
                row.observed
                    .map_or_else(|| "-".to_string(), |ts| ts.to_rfc3339()),
                row.reported_at.clone().unwrap_or_else(|| "-".to_string()),
                row.path.clone().unwrap_or_else(|| "-".to_string()),
            ]
        })
        .collect();
    table::print(
        &["HOST", "KIND", "AGE", "OBSERVED", "REPORTED_AT", "BEACON"],
        &table_rows,
    );
    Ok(())
}
