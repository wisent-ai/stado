//! Demand no host in the fleet can serve: a platform nobody declared, a GPU
//! bigger or of a kind the fleet does not have, or the only GPU held by
//! something outside the queue.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{diag_number, Evidence, Need, NeedKind, Severity};
use crate::fleet_needs::unmet::UnmetPlacement;
use crate::models::Job;
use crate::primitives::tally::Tally;
use crate::queue::capacity::Publication;
use crate::targets::Registry;

pub(super) fn platform_needs(
    registry: &Registry,
    unmet: &[UnmetPlacement],
    queued: &[Job],
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
            demand.entry(platform.to_string()).or_default().0.tally();
        }
    }
    for job in queued {
        if job.platform_os.is_empty() {
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
            .1
            .tally();
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
                    format!("{waiting} queued job(s) require {platform}"),
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
    // The largest VRAM any live host publishes; none published is not a
    // host with no VRAM, and every request for VRAM is then unmet.
    let largest_vram = publications
        .values()
        .filter(|publication| !publication.stale(now))
        .filter_map(|publication| {
            publication
                .payload
                .get("total_vram_gb")
                .and_then(Value::as_i64)
        })
        .max();
    let beyond_fleet = |wanted: i64| match largest_vram {
        Some(largest) => wanted > largest,
        None => wanted.is_positive(),
    };
    let gpu_types: Vec<String> = registry
        .targets
        .iter()
        .filter_map(|target| target.gpu_type.clone())
        .collect();
    let mut wants: Vec<String> = queued
        .iter()
        .filter(|job| beyond_fleet(job.gpu_mem_gb))
        .map(|job| format!("{} wants {} GiB VRAM", job.job_id, job.gpu_mem_gb))
        .collect();
    wants.extend(
        unmet
            .iter()
            .filter(|record| beyond_fleet(record.requirement.vram_gb))
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
            .filter(|job| !job.gpu_type.is_empty() && !gpu_types.contains(&job.gpu_type))
            .map(|job| format!("{} wants gpu_type {}", job.job_id, job.gpu_type)),
    );
    wants.extend(
        unmet
            .iter()
            .filter_map(|record| record.requirement.gpu_type.as_deref())
            .filter(|kind| !gpu_types.iter().any(|have| have == kind))
            .map(|kind| format!("a placement asked for gpu_type {kind}")),
    );
    // A GPU is held outside the queue only when its host published all three
    // readings: total, none free, and VRAM no Stado job accounts for.
    let held = publications.iter().find_map(|(consumer, publication)| {
        let payload = &publication.payload;
        let total = payload.get("total_vram_gb").and_then(Value::as_i64)?;
        let free = payload.get("free_vram_gb").and_then(Value::as_i64)?;
        let unattributed = diag_number(payload, "vram_unattributed_gb")?;
        let held_outside = unattributed.is_sign_positive() && unattributed.is_normal();
        (!publication.stale(now) && total.is_positive() && free.counts_nothing() && held_outside).then(|| {
            format!(
                "{consumer} publishes {free} of {total} GiB VRAM free with {unattributed:.1} GiB held by no Stado job"
            )
        })
    });
    if wants.is_empty() && held.is_none() {
        return Vec::new();
    }
    let mut evidence: Vec<Evidence> = wants
        .iter()
        .map(|detail| Evidence::new("queue", detail.clone()))
        .collect();
    if let Some(detail) = held.clone() {
        evidence.push(Evidence::new("capacity", detail));
    }
    let largest_published = match largest_vram {
        Some(largest) => format!("{largest} GiB"),
        None => "not published by any live host".to_string(),
    };
    evidence.push(Evidence::new(
        "registry",
        format!(
            "largest published VRAM {largest_published}; declared gpu types: {}",
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
            format!(
                "{} request(s) need a GPU the fleet does not have",
                wants.len()
            )
        },
        evidence,
        suggestion: if only_held {
            "free the GPU (see `stado space report` on that host for who holds it) or add a second GPU host".to_string()
        } else {
            "add a GPU host with more memory or the requested type".to_string()
        },
    }]
}
