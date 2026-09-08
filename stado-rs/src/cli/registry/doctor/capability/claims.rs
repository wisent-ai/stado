//! The capability half of `registry doctor`: what each declared service
//! claims to run, judged against the published roster and the last
//! measurement of the host it runs on.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};

use crate::cli::registry::doctor::capability::gaps::{unmet_requirements, unplaced_jobs};
use crate::cli::registry::doctor::capability::requirements::{
    declared_trajectories, load_job_requirements, RequirementClaim, REQUIREMENTS_PREFIX,
};
use crate::cli::registry::doctor::capability::{load_capability_measurements, CAPABILITIES_PREFIX};
use crate::cli::registry::doctor::findings::Finding;
use crate::queue::JobStorage;
use crate::targets::{ComputeTarget, Registry};

/// Append every capability finding this document earns, and return the three
/// counts `doctor` reports as `requirement_claims`, `declared_trajectories`
/// and `capability_measurements`.
pub(in crate::cli::registry::doctor) async fn requirement_findings(
    store: &JobStorage,
    registry: &Registry,
    now: DateTime<Utc>,
    findings: &mut Vec<Finding>,
) -> (usize, usize, usize) {
    // Which declared service runs which job. The published roster is read whether
    // or not anything declares one, because a job the roster names and no target
    // declares is itself a finding: a declared service entry is the result of a
    // placement that succeeded, so a job with no entry anywhere is a job waiting
    // for a host that can take it.
    let claims: Vec<(&ComputeTarget, RequirementClaim)> = registry
        .targets
        .iter()
        .flat_map(|target| {
            declared_trajectories(target)
                .into_iter()
                .map(move |claim| (target, claim))
        })
        .collect();
    let mut measured_hosts = usize::default();
    let mut roster = usize::default();
    // An unreadable prefix is not an absent object, and reporting it as one would
    // say a host cannot do something when the truth is that nobody here may look.
    // Both reads share that reasoning, so both report the store's own words.
    match (
        load_job_requirements(store, now).await,
        load_capability_measurements(store).await,
    ) {
        (Ok(declarations), Ok(measurements)) => {
            measured_hosts = measurements.len();
            roster = declarations.needs.len();
            for (target, claim) in &claims {
                for reason in unmet_requirements(
                    &target.name,
                    claim,
                    &declarations,
                    measurements.get(&target.name),
                    now,
                ) {
                    findings.push(
                        Finding::new("capability-unsatisfied", &target.name, reason)
                            .about(claim.label.as_str()),
                    );
                }
            }
            let placed: BTreeSet<&str> = claims
                .iter()
                .map(|(_, claim)| claim.trajectory.as_str())
                .collect();
            findings.extend(unplaced_jobs(
                registry,
                &declarations,
                &measurements,
                &placed,
                now,
            ));
        }
        // One row, not one per claim: the cause is a store that will not answer,
        // and it is the same cause for every job.
        (declarations, measurements) => {
            let refused = [
                declarations
                    .err()
                    .map(|exc| format!("{REQUIREMENTS_PREFIX}/ could not be read: {exc}")),
                measurements
                    .err()
                    .map(|exc| format!("{CAPABILITIES_PREFIX}/ could not be read: {exc}")),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<String>>()
            .join("; ");
            findings.push(Finding::new(
                "capability-unsatisfied",
                format!("{REQUIREMENTS_PREFIX}/"),
                format!(
                    "{} declared trajectory claim(s) cannot be judged and no job can be placed: \
                     {refused}",
                    claims.len()
                ),
            ));
        }
    }
    (claims.len(), roster, measured_hosts)
}
