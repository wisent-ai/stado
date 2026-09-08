//! What stops a measured host from satisfying what it is declared to run,
//! and the job nothing runs because no host could take it.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};

use crate::cli::registry::beacons::stale_after_seconds;
use crate::cli::registry::doctor::capability::requirements::{Declarations, RequirementClaim};
use crate::cli::registry::doctor::capability::{
    Measurement, CAPABILITIES_PREFIX, CAPABILITIES_SCHEMA,
};
use crate::cli::registry::doctor::findings::Finding;
use crate::cli::registry::human_age;
use crate::targets::Registry;

/// What stops one host's last measurement from satisfying a capability list, one
/// clause per reason, empty when nothing does.
///
/// The single place that answers "can this host run this job", so the finding
/// about a host that declares the job and the finding about a job no host
/// declares cannot answer it differently. A missing, mis-schema'd or stale
/// measurement disqualifies the host in one clause rather than once per
/// capability: the operator's next action is the same however many ids were
/// named, and it is to go and measure the host.
fn measurement_gaps(
    target: &str,
    capabilities: &[String],
    measurement: Option<&Measurement>,
    now: DateTime<Utc>,
) -> Vec<String> {
    // A job that needs nothing of the host is satisfied by every host, measured
    // or not: `codex/reauth` declares exactly that.
    if capabilities.is_empty() {
        return Vec::new();
    }
    let Some(measurement) = measurement else {
        return vec![format!(
            "{CAPABILITIES_PREFIX}/{target}.json does not exist: nothing has measured this host"
        )];
    };
    if measurement.schema != CAPABILITIES_SCHEMA {
        return vec![format!(
            "{} carries schema {:?} rather than {CAPABILITIES_SCHEMA}",
            measurement.path, measurement.schema
        )];
    }
    match measurement.measured_at {
        None => {
            return vec![format!(
                "{} carries neither measured_at nor an object timestamp, so its age cannot be \
                 judged",
                measurement.path
            )]
        }
        Some(measured) => {
            let age = now - measured;
            if age.num_seconds() > stale_after_seconds() {
                return vec![format!(
                    "{} was measured {} ago ({}), past the {}s liveness window",
                    measurement.path,
                    human_age(age),
                    measured.to_rfc3339(),
                    stale_after_seconds()
                )];
            }
        }
    }
    capabilities
        .iter()
        .filter_map(
            |capability| match measurement.capabilities.get(capability) {
                None => Some(format!(
                    "{} does not measure {capability}",
                    measurement.path
                )),
                Some(measured) if !measured.value => {
                    Some(format!("{capability} measured false: {}", measured.detail))
                }
                Some(_) => None,
            },
        )
        .collect()
}

/// Every hop of the join that fails for one declared service, one sentence each:
/// declared service -> trajectory id -> published requirement -> measured
/// capability. Empty means the host is measured able to run what it is declared to
/// run.
pub(super) fn unmet_requirements(
    target: &str,
    claim: &RequirementClaim,
    declarations: &Declarations,
    measurement: Option<&Measurement>,
    now: DateTime<Utc>,
) -> Vec<String> {
    let unit = &claim.unit;
    let trajectory = &claim.trajectory;
    let Some((source, capabilities)) = declarations.needs.get(trajectory) else {
        return vec![format!(
            "{unit} runs trajectory {trajectory}, and no published declaration names it: {}",
            declarations.consulted()
        )];
    };
    let needs = capabilities.join(", ");
    measurement_gaps(target, capabilities, measurement, now)
        .into_iter()
        .map(|gap| {
            format!("{unit} runs {trajectory}, which {source} says requires {needs}, and {gap}")
        })
        .collect()
}

/// Jobs in the published roster that nothing runs and no host could.
///
/// A declared service entry is the RESULT of a placement that succeeded, so a
/// trajectory no target declares is a job waiting for a host. That is only worth
/// reporting when no candidate can take it: while some measured host satisfies the
/// requirement, placement has an answer and the absence is a step not yet taken
/// rather than a contradiction. The row names each candidate and the measurement
/// that disqualified it, so it says what would have to change, and it disappears by
/// itself the moment a capable host exists and the unit is adopted where it runs.
pub(super) fn unplaced_jobs(
    registry: &Registry,
    declarations: &Declarations,
    measurements: &BTreeMap<String, Measurement>,
    placed: &BTreeSet<&str>,
    now: DateTime<Utc>,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (trajectory, (source, capabilities)) in &declarations.needs {
        if capabilities.is_empty() || placed.contains(trajectory.as_str()) {
            continue;
        }
        let mut disqualified = Vec::new();
        for target in &registry.targets {
            // Only kind=local names a machine that can hold a session and run a
            // browser; "gcp" and "vast" targets are dispatcher pools.
            if !target.is_provider(crate::capabilities::ProviderId::Local) {
                continue;
            }
            let gaps = measurement_gaps(
                &target.name,
                capabilities,
                measurements.get(&target.name),
                now,
            );
            if gaps.is_empty() {
                disqualified.clear();
                break;
            }
            disqualified.push(format!("{}: {}", target.name, gaps.join(", ")));
        }
        if !disqualified.is_empty() {
            findings.push(Finding::new(
                "job-unplaced",
                trajectory,
                format!(
                    "{source} says it requires {}, no registry target declares a service that \
                     runs it, and no host can take it — {}",
                    capabilities.join(", "),
                    disqualified.join("; ")
                ),
            ));
        }
    }
    findings
}
