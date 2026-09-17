//! Demand no host in the fleet can serve: a platform nobody declared, a GPU
//! bigger or of a kind the fleet does not have, or the only GPU held by
//! something outside the queue.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{diag_number, is_stale, Evidence, Need, NeedKind, Severity, STALE_QUEUE_SECONDS};
use crate::fleet_needs::unmet::UnmetPlacement;
use crate::models::Job;
use crate::queue::capacity::Publication;
use crate::targets::Registry;

pub(super) fn platform_needs(
    registry: &Registry,
    unmet: &[UnmetPlacement],
    queued: &[Job],
    now: DateTime<Utc>,
) -> Vec<Need> {
    let declared: Vec<&str> = registry
        .targets
        .iter()
        .map(|target| target.release_platform.as_str())
        .collect();
    // (refused placements, waiting jobs) per wanted platform.
    let mut demand: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for record in unmet {
        if let Some(platform) = record.requirement.platform.as_deref() {
            demand.entry(platform.to_string()).or_default().0 += 1;
        }
    }
    for job in queued {
        if job.platform_os.is_empty() || !is_stale(job, now) {
            continue;
        }
        let architecture = if job.architecture.is_empty() {
            "*"
        } else {
            job.architecture.as_str()
        };
        demand
            .entry(format!("{}-{architecture}", job.platform_os))
            .or_default()
            .1 += 1;
    }
    demand
        .into_iter()
        .filter(|(platform, _)| !declared.iter().any(|have| platform_matches(have, platform)))
        .map(|(platform, (refused, waiting))| Need {
            need: NeedKind::Host,
            target: None,
            platform: Some(platform.clone()),
            severity: Severity::High,
            summary: format!(
                "no declared host runs {platform}; {refused} placement(s) refused and {waiting} queued job(s) waiting for it"
            ),
            evidence: vec![
                Evidence::new(
                    "registry",
                    format!("declared platforms: {}", declared.join(", ")),
                ),
                Evidence::new(
                    "unmet",
                    format!("{refused} unmet placement record(s) name platform {platform}"),
                ),
                Evidence::new(
                    "queue",
                    format!(
                        "{waiting} queued job(s) older than {} minutes require {platform}",
                        STALE_QUEUE_SECONDS / 60
                    ),
                ),
            ],
            suggestion: format!(
                "add a {platform} machine to the fleet and register it with `stado registry host add`"
            ),
        })
        .collect()
}

fn platform_matches(declared: &str, wanted: &str) -> bool {
    match wanted.strip_suffix("-*") {
        Some(os) => declared.starts_with(os),
        None => declared == wanted,
    }
}

pub(super) fn gpu_needs(
    registry: &Registry,
    publications: &BTreeMap<String, Publication>,
    unmet: &[UnmetPlacement],
    queued: &[Job],
    now: DateTime<Utc>,
) -> Vec<Need> {
    let largest_vram = publications
        .values()
        .filter(|publication| !publication.stale(now))
        .filter_map(|publication| {
            publication
                .payload
                .get("total_vram_gb")
                .and_then(Value::as_i64)
        })
        .max()
        .unwrap_or(0);
    let gpu_types: Vec<String> = registry
        .targets
        .iter()
        .filter_map(|target| target.gpu_type.clone())
        .collect();
    let mut wants: Vec<String> = queued
        .iter()
        .filter(|job| is_stale(job, now) && job.gpu_mem_gb > largest_vram)
        .map(|job| format!("{} wants {} GiB VRAM", job.job_id, job.gpu_mem_gb))
        .collect();
    wants.extend(
        unmet
            .iter()
            .filter(|record| record.requirement.vram_gb > largest_vram)
            .map(|record| {
                format!(
                    "{} asked for {} GiB VRAM",
                    record.kind, record.requirement.vram_gb
                )
            }),
    );
    wants.extend(
        queued
            .iter()
            .filter(|job| {
                is_stale(job, now) && !job.gpu_type.is_empty() && !gpu_types.contains(&job.gpu_type)
            })
            .map(|job| format!("{} wants gpu_type {}", job.job_id, job.gpu_type)),
    );
    wants.extend(
        unmet
            .iter()
            .filter_map(|record| record.requirement.gpu_type.as_deref())
            .filter(|kind| !gpu_types.iter().any(|have| have == kind))
            .map(|kind| format!("a placement asked for gpu_type {kind}")),
    );
    let held = publications.iter().find_map(|(consumer, publication)| {
        let payload = &publication.payload;
        let total = payload
            .get("total_vram_gb")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let free = payload
            .get("free_vram_gb")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let unattributed = diag_number(payload, "vram_unattributed_gb").unwrap_or(0.0);
        (!publication.stale(now) && total > 0 && free == 0 && unattributed > 0.0).then(|| {
            format!(
                "{consumer} publishes 0 of {total} GiB VRAM free with {unattributed:.1} GiB held by no Stado job"
            )
        })
    });
    if wants.is_empty() && held.is_none() {
        return Vec::new();
    }
    let mut evidence: Vec<Evidence> = wants
        .iter()
        .map(|detail| Evidence::new(
            "queue",
            detail.clone(),
        ))
        .collect();
    if let Some(detail) = held.clone() {
        evidence.push(Evidence::new(
            "capacity",
            detail,
        ));
    }
    evidence.push(Evidence::new(
        "registry",
        format!(
            "largest published VRAM {largest_vram} GiB; declared gpu types: {}",
            if gpu_types.is_empty() {
                "none".to_string()
            } else {
                gpu_types.join(", ")
            }
        ),
    ));
    let only_held = held.is_some() && wants.is_empty();
    vec![Need {
        need: NeedKind::Gpu,
        target: None,
        platform: None,
        severity: Severity::High,
        summary: if only_held {
            "the only GPU is fully held by something outside the queue".to_string()
        } else {
            format!("{} request(s) need a GPU the fleet does not have", wants.len())
        },
        evidence,
        suggestion: if only_held {
            "free the GPU (see `stado space report` on that host for who holds it) or add a second GPU host".to_string()
        } else {
            "add a GPU host with more memory or the requested type".to_string()
        },
    }]
}
