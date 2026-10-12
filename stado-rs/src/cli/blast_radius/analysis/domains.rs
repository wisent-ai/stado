//! Data domains: the canonical prefixes grouped by the consumers that break
//! together, so an incident scope is never reported as one flat object count.

use std::collections::BTreeSet;

use crate::cli::blast_radius::{
    DomainReport, Reach, StorageInspection, AUTOMATION, FLEET_OBSERVABILITY, JOB_LIFECYCLE,
    PAYLOADS, REGISTRY, SCHEDULER_CONTROL,
};

pub(in crate::cli::blast_radius) fn data_domains(primary: &StorageInspection) -> Vec<DomainReport> {
    vec![
        domain(
            "job_lifecycle",
            JOB_LIFECYCLE,
            "the scheduler, workers, status, results, cancel, and desktop and API job views",
            primary,
        ),
        domain(
            "scheduler_control",
            SCHEDULER_CONTROL,
            "the coordinator, scheduler, quota reservations and recurring jobs",
            primary,
        ),
        domain(
            "fleet_observability",
            FLEET_OBSERVABILITY,
            "the dashboard, overview, host health and billing health",
            primary,
        ),
        domain(
            "automation_requests",
            AUTOMATION,
            "the machine API and automation clients",
            primary,
        ),
        domain(
            "runs_artifacts_and_scripts",
            PAYLOADS,
            "run history, artifact consumers and worker startup scripts",
            primary,
        ),
        domain(
            "registry",
            REGISTRY,
            "coordinators and host commands",
            primary,
        ),
    ]
}

fn domain(
    name: &str,
    prefixes: &[&str],
    consumers: &str,
    storage: &StorageInspection,
) -> DomainReport {
    let count = if storage.report.state == Reach::Reachable {
        Some(
            prefixes
                .iter()
                .filter_map(|prefix| storage.names.get(*prefix))
                .map(BTreeSet::len)
                .sum(),
        )
    } else {
        None
    };
    DomainReport {
        domain: name.to_string(),
        object_count: count,
        prefixes: prefixes.iter().map(|value| (*value).to_string()).collect(),
        consumers: consumers.to_string(),
    }
}
