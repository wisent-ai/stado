//! Close the gap between what the registry declares a host must run and what
//! that host actually runs.
//!
//! Every piece of this existed and none of them were joined up.
//! `ComputeTarget::managed_versions` is the declaration. `host inventory`
//! reads each host's installed binary and already computes the axis - behind,
//! ahead, mismatched, unjudged, undeclared. `host release` fetches an exact
//! coordinate, verifies its digest, stages it and repoints the active binary.
//! What no command did was walk the fleet, put those three together, and say
//! what is out of step.
//!
//! The consequence was not theoretical. A subcommand merged to `main` reached
//! no machine at all, and every host answered that its binary predated the
//! command, because nothing existed whose job was to notice.
//!
//! This deliberately does not reclassify anything. The verdicts come from
//! `host inventory`'s own report: a second implementation of "is this host
//! behind" is a second answer to one question, and the two would drift.
//!
//! Reporting and acting are separate. The default is a report, because an
//! operator has to be able to see drift without a machine changing under
//! them; delivery is a second, explicit act.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::host_inventory::VersionVerdict;
use super::{host_inventory, DeployError, Runner};

/// How one managed binary on a host differs from its declaration. Built
/// from `host inventory`'s own [`VersionVerdict`] by an exhaustive match, so
/// a verdict added there has to be placed here before this compiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Drift {
    /// Older than declared.
    Behind,
    /// Newer than declared.
    Ahead,
    /// Different, with no order between the two versions.
    Mismatched,
    /// Declared, and no installed version could be read.
    Unknown,
    /// Declared and not on the host: the most actionable state there is,
    /// and the one an "is it the right version" question skips over.
    Absent,
}

impl Drift {
    /// A delivery of the declared version is what brings this binary back.
    pub fn delivers(self) -> bool {
        matches!(self, Self::Behind | Self::Absent)
    }
}

/// What one managed binary on one host is doing.
#[derive(Debug, Clone)]
pub struct Standing {
    pub binary: String,
    pub verdict: Drift,
    pub declared: String,
    pub installed: String,
}

/// One host's standing against its declaration.
#[derive(Debug, Clone)]
pub struct HostStanding {
    pub target: String,
    pub release_platform: String,
    pub declared_release_platform: String,
    pub platform_verdict: String,
    pub drift: Vec<Standing>,
    pub undeclared: Vec<String>,
    pub unreachable: Option<String>,
}

impl HostStanding {
    pub fn needs_delivery(&self) -> bool {
        self.unreachable.is_none()
            && self.platform_verdict == host_inventory::MATCHED
            && self.drift.iter().any(|entry| entry.verdict.delivers())
    }

    pub fn settled(&self) -> bool {
        self.unreachable.is_none()
            && self.platform_verdict == host_inventory::MATCHED
            && self.drift.is_empty()
            && self.undeclared.is_empty()
    }
}

fn text(entry: &Value, field: &str) -> String {
    entry
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Each managed binary's drift from the inventory report, and the binaries
/// the registry declares nothing for. A binary without a `version_verdict`,
/// or with one `host inventory` does not write, is refused by name.
fn drift_of(report: &Value) -> Result<(Vec<Standing>, Vec<String>), String> {
    let mut drift = Vec::new();
    let mut undeclared = Vec::new();
    let Some(binaries) = report.get("managed_binaries").and_then(Value::as_array) else {
        return Ok((drift, undeclared));
    };
    for entry in binaries {
        let name = text(entry, "name");
        let raw = entry
            .get("version_verdict")
            .ok_or_else(|| format!("host inventory reported {name} without a version_verdict"))?;
        let verdict = VersionVerdict::deserialize(raw).map_err(|error| {
            format!("host inventory reported version_verdict {raw} for {name}, which it does not write: {error}")
        })?;
        let present = text(entry, "state") == "present";
        let verdict = match verdict {
            VersionVerdict::Matched => continue,
            VersionVerdict::Undeclared => {
                undeclared.push(name);
                continue;
            }
            _ if !present => Drift::Absent,
            VersionVerdict::Behind => Drift::Behind,
            VersionVerdict::Ahead => Drift::Ahead,
            VersionVerdict::Mismatched => Drift::Mismatched,
            VersionVerdict::Unknown => Drift::Unknown,
        };
        drift.push(Standing {
            binary: name,
            verdict,
            declared: text(entry, "declared_version"),
            installed: text(entry, "version"),
        });
    }
    Ok((drift, undeclared))
}

/// Read one host and record where each managed binary stands.
///
/// The verdict is `host inventory`'s own, taken per binary from its report
/// rather than recomputed here. Two implementations of "is this host behind"
/// are two answers to one question, and they drift apart exactly when it
/// matters.
pub async fn examine(target_name: &str, runner: &Runner) -> HostStanding {
    match host_inventory::inventory_host(target_name, runner).await {
        Ok(report)
            if report.get("status").and_then(Value::as_str) == Some(host_inventory::OK_STATUS) =>
        {
            let (drift, undeclared, unreachable) = match drift_of(&report) {
                Ok((drift, undeclared)) => (drift, undeclared, None),
                Err(malformed) => (Vec::new(), Vec::new(), Some(malformed)),
            };
            HostStanding {
                target: target_name.to_string(),
                release_platform: text(&report, "release_platform"),
                declared_release_platform: text(&report, "declared_release_platform"),
                platform_verdict: text(&report, "release_platform_verdict"),
                drift,
                undeclared,
                unreachable,
            }
        }
        Ok(report) => HostStanding {
            target: target_name.to_string(),
            release_platform: String::new(),
            declared_release_platform: String::new(),
            platform_verdict: String::new(),
            drift: Vec::new(),
            undeclared: Vec::new(),
            unreachable: Some(
                report
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("host inventory failed without a detail")
                    .to_string(),
            ),
        },
        Err(DeployError {
            message: detail, ..
        }) => HostStanding {
            target: target_name.to_string(),
            release_platform: String::new(),
            declared_release_platform: String::new(),
            platform_verdict: String::new(),
            drift: Vec::new(),
            undeclared: Vec::new(),
            unreachable: Some(detail),
        },
    }
}

/// The fleet's standing, as JSON an operator or another tool can read.
pub fn report(standings: &[HostStanding], deliveries: &[Value]) -> Value {
    let mut drifting = 0_usize;
    let mut unreachable = 0_usize;
    let mut undeclared = 0_usize;
    let mut platform_mismatches = 0_usize;
    let hosts: Vec<Value> = standings
        .iter()
        .map(|standing| {
            unreachable += usize::from(standing.unreachable.is_some());
            drifting += usize::from(!standing.drift.is_empty());
            undeclared += standing.undeclared.len();
            platform_mismatches += usize::from(
                standing.unreachable.is_none()
                    && standing.platform_verdict != host_inventory::MATCHED,
            );
            json!({
                "target": standing.target,
                "release_platform": standing.release_platform,
                "declared_release_platform": standing.declared_release_platform,
                "platform_verdict": standing.platform_verdict,
                "unreachable": standing.unreachable,
                "undeclared": standing.undeclared,
                "drift": standing
                    .drift
                    .iter()
                    .map(|entry| json!({
                        "binary": entry.binary,
                        "verdict": entry.verdict,
                        "declared": entry.declared,
                        "installed": entry.installed,
                    }))
                    .collect::<Vec<Value>>(),
            })
        })
        .collect();
    let failed_deliveries = deliveries
        .iter()
        .filter(|entry| entry.get("status").and_then(Value::as_str) != Some("delivered"))
        .count();
    json!({
        "summary": {
            "hosts": standings.len(),
            "settled": standings.iter().filter(|standing| standing.settled()).count(),
            "drifting": drifting,
            "unreachable": unreachable,
            "platform_mismatches": platform_mismatches,
            "undeclared_binaries": undeclared,
            "deliveries": deliveries.len(),
            "failed_deliveries": failed_deliveries,
        },
        "hosts": hosts,
        "deliveries": deliveries,
    })
}
