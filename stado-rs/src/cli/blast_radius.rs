//! `stado blast-radius` — side-effect-free incident scope and DR readiness.
//!
//! The command answers four separate questions instead of collapsing them
//! into "the queue is empty": whether the configured primary store can be
//! listed, which state domains and consumers depend on the failed service,
//! whether a separately configured backup can be read, and how closely that
//! backup covers the primary namespace when both ends are reachable.
//!
//! A backup is never selected automatically here. Queue state contains CAS
//! locks, leases and moving job records; transparent read fallback can make
//! two schedulers dispatch the same work from divergent stores. Promotion
//! must fence writers first, then select one backend for every participant.

use std::collections::{BTreeMap, BTreeSet};

use crate::config;
use chrono::{DateTime, SecondsFormat, Utc};
use clap::Args;
use serde::Serialize;

use crate::queue::copy::{Endpoint, CANONICAL_PREFIXES};
use crate::queue::BlobBackend;

use super::CmdError;

#[derive(Args, Debug)]
pub struct BlastRadiusArgs {
    /// Failed dependency to assess: gcp, azure, aws or local.
    #[arg(long, default_value = "gcp")]
    dependency: String,
    /// Emit the complete machine-readable report.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Serialize)]
struct PrefixReport {
    prefix: String,
    object_count: Option<usize>,
    newest_object_at: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct StorageReport {
    role: String,
    locator: Option<String>,
    state: String,
    object_count: Option<usize>,
    newest_object_at: Option<String>,
    error: Option<String>,
    prefixes: Vec<PrefixReport>,
}

struct StorageInspection {
    report: StorageReport,
    names: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Debug, Serialize)]
struct DomainReport {
    domain: String,
    object_count: Option<usize>,
    prefixes: Vec<String>,
    consumers: Vec<String>,
}

#[derive(Debug, Serialize)]
struct CoverageReport {
    state: String,
    missing_from_backup: Option<usize>,
    extra_only_in_backup: Option<usize>,
    explanation: String,
}

#[derive(Debug, Serialize)]
struct DownstreamImpact {
    component: String,
    severity: String,
    state: String,
    data: Vec<String>,
    consumers: Vec<String>,
    reason: String,
}

#[derive(Debug, Serialize)]
struct FailoverPolicy {
    automatic: bool,
    safe_mode: String,
    reason: String,
}

#[derive(Debug, Serialize)]
struct Summary {
    state: String,
    affected_components: usize,
    primary_objects_in_scope: Option<usize>,
    backup_objects_in_scope: Option<usize>,
    scale_source: String,
}

#[derive(Debug, Serialize)]
struct BlastRadiusReport {
    dependency: String,
    configured_storage_backend: String,
    configured_compute_providers: Vec<String>,
    summary: Summary,
    primary_storage: StorageReport,
    backup_storage: StorageReport,
    backup_coverage: CoverageReport,
    data_domains: Vec<DomainReport>,
    downstream: Vec<DownstreamImpact>,
    failover: FailoverPolicy,
    recovery_order: Vec<String>,
}

const JOB_LIFECYCLE: &[&str] = &[
    "queue/",
    "running/",
    "completed/",
    "uploaded/",
    "failed/",
    "cancelled/",
    "cancellations/",
];
const SCHEDULER_CONTROL: &[&str] = &[
    "queue_priority/",
    "provider-leases/",
    "schedules/",
    "config/",
    "state/",
    "failure_fixes/",
    "fixed/",
    "failed_again/",
    "coverage/",
    "hf_rate/",
];
const FLEET_OBSERVABILITY: &[&str] = &["status/", "capacity/", "host_health/", "billing_health/"];
const AUTOMATION: &[&str] = &["machine_requests/", "machine_inputs/"];
const PAYLOADS: &[&str] = &["runs/", "scripts/", "artifacts/"];
const CONTROL_AND_SECRETS: &[&str] = &["registry.json", "secrets/"];

pub async fn run(args: &BlastRadiusArgs) -> Result<(), CmdError> {
    validate_dependency(&args.dependency)?;

    let primary_endpoint = Endpoint::configured_primary();
    let primary = inspect_storage("primary", Some(&primary_endpoint)).await;
    let backup_endpoint = Endpoint::configured_backup();
    let backup_matches_primary = backup_endpoint
        .as_ref()
        .is_some_and(|endpoint| endpoint.describe() == primary_endpoint.describe());
    let backup = inspect_storage("backup", backup_endpoint.as_ref()).await;
    let coverage = compare_coverage(
        &primary,
        &backup,
        backup_endpoint.is_some(),
        backup_matches_primary,
    );
    let domains = data_domains(&primary);
    let downstream = downstream_impacts(&args.dependency, &primary.report, &backup.report);
    let affected_components = downstream
        .iter()
        .filter(|impact| impact.state != "unaffected")
        .count();
    let primary_unavailable = primary.report.state != "reachable";
    let dependency_owns_primary =
        dependency_owns_backend(&args.dependency, config::wc_storage_backend());
    let state = if dependency_owns_primary && primary_unavailable {
        "critical_outage"
    } else if dependency_owns_primary {
        "primary_at_risk"
    } else if affected_components == usize::default() {
        "unaffected"
    } else {
        "degraded"
    };
    let (scale_source, primary_scope, backup_scope) = if primary.report.object_count.is_some() {
        (
            "primary_listing".to_string(),
            primary.report.object_count,
            backup.report.object_count,
        )
    } else if backup.report.object_count.is_some() {
        (
            "backup_listing_primary_unknown".to_string(),
            None,
            backup.report.object_count,
        )
    } else {
        ("no_readable_store".to_string(), None, None)
    };

    let report = BlastRadiusReport {
        dependency: args.dependency.clone(),
        configured_storage_backend: config::wc_storage_backend().to_string(),
        configured_compute_providers: config::wc_providers().to_vec(),
        summary: Summary {
            state: state.to_string(),
            affected_components,
            primary_objects_in_scope: primary_scope,
            backup_objects_in_scope: backup_scope,
            scale_source,
        },
        primary_storage: primary.report,
        backup_storage: backup.report,
        backup_coverage: coverage,
        data_domains: domains,
        downstream,
        failover: FailoverPolicy {
            automatic: false,
            safe_mode: "fence_writers_then_explicitly_promote_one_backend".to_string(),
            reason: "queue records, provider leases and compare-and-swap state are mutable; transparent fallback risks duplicate dispatch and split brain".to_string(),
        },
        recovery_order: [
            "establish one readable authoritative store",
            "fence every scheduler and worker writer",
            "verify backup namespace and metadata",
            "promote the selected backend to every participant",
            "restore registry, secrets and release distribution",
            "resume coordinators and workers",
            "verify queue lifecycle and user-facing consumers",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
    };

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|error| CmdError::click(error.to_string()))?
        );
    } else {
        print_human(&report);
    }
    Ok(())
}

fn validate_dependency(dependency: &str) -> Result<(), CmdError> {
    match dependency {
        "gcp" | "azure" | "aws" | "local" => Ok(()),
        other => Err(CmdError::click(format!(
            "unknown dependency {other:?}; use gcp, azure, aws or local"
        ))),
    }
}

async fn inspect_storage(role: &str, endpoint: Option<&Endpoint>) -> StorageInspection {
    let Some(endpoint) = endpoint else {
        return StorageInspection {
            report: StorageReport {
                role: role.to_string(),
                locator: None,
                state: "not_configured".to_string(),
                object_count: None,
                newest_object_at: None,
                error: None,
                prefixes: Vec::new(),
            },
            names: BTreeMap::new(),
        };
    };

    let locator = endpoint.describe();
    let backend = match endpoint.build().await {
        Ok(backend) => backend,
        Err(error) => {
            return StorageInspection {
                report: StorageReport {
                    role: role.to_string(),
                    locator: Some(locator),
                    state: "unreachable".to_string(),
                    object_count: None,
                    newest_object_at: None,
                    error: Some(error.to_string()),
                    prefixes: Vec::new(),
                },
                names: BTreeMap::new(),
            }
        }
    };

    inspect_backend(role, locator, &backend).await
}

async fn inspect_backend(
    role: &str,
    locator: String,
    backend: &std::sync::Arc<dyn BlobBackend>,
) -> StorageInspection {
    let mut reports = Vec::new();
    let mut names = BTreeMap::new();
    let mut newest = None;
    let mut total = usize::default();
    let mut first_error = None;

    for prefix in CANONICAL_PREFIXES {
        match backend.list_blobs_with_meta(prefix).await {
            Ok(blobs) => {
                let prefix_newest = blobs
                    .iter()
                    .filter_map(|blob| blob.updated.as_ref().cloned())
                    .max();
                newest = max_stamp(newest, prefix_newest);
                total = total.saturating_add(blobs.len());
                names.insert(
                    (*prefix).to_string(),
                    blobs.iter().map(|blob| blob.name.clone()).collect(),
                );
                reports.push(PrefixReport {
                    prefix: (*prefix).to_string(),
                    object_count: Some(blobs.len()),
                    newest_object_at: render_stamp(prefix_newest),
                    error: None,
                });
            }
            Err(error) => {
                let message = error.to_string();
                if first_error.is_none() {
                    first_error = Some(message.clone());
                }
                reports.push(PrefixReport {
                    prefix: (*prefix).to_string(),
                    object_count: None,
                    newest_object_at: None,
                    error: Some(message),
                });
                break;
            }
        }
    }

    let reachable = first_error.is_none();
    StorageInspection {
        report: StorageReport {
            role: role.to_string(),
            locator: Some(locator),
            state: if reachable {
                "reachable"
            } else {
                "unreachable"
            }
            .to_string(),
            object_count: reachable.then_some(total),
            newest_object_at: render_stamp(newest),
            error: first_error,
            prefixes: reports,
        },
        names,
    }
}

fn max_stamp(left: Option<DateTime<Utc>>, right: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(stamp), None) | (None, Some(stamp)) => Some(stamp),
        (None, None) => None,
    }
}

fn render_stamp(stamp: Option<DateTime<Utc>>) -> Option<String> {
    stamp.map(|value| value.to_rfc3339_opts(SecondsFormat::Secs, true))
}

fn compare_coverage(
    primary: &StorageInspection,
    backup: &StorageInspection,
    configured: bool,
    matches_primary: bool,
) -> CoverageReport {
    if !configured {
        return CoverageReport {
            state: "not_configured".to_string(),
            missing_from_backup: None,
            extra_only_in_backup: None,
            explanation: "no disaster-recovery endpoint is configured; queue state has no Stado-managed backup".to_string(),
        };
    }
    if matches_primary {
        return CoverageReport {
            state: "same_as_primary_not_a_backup".to_string(),
            missing_from_backup: None,
            extra_only_in_backup: None,
            explanation: "the backup locator resolves to the primary store and provides no independent failure domain".to_string(),
        };
    }
    if backup.report.state != "reachable" {
        return CoverageReport {
            state: "backup_unreadable".to_string(),
            missing_from_backup: None,
            extra_only_in_backup: None,
            explanation: "the backup endpoint is configured but could not be listed".to_string(),
        };
    }
    if primary.report.state != "reachable" {
        return CoverageReport {
            state: "backup_readable_primary_unknown".to_string(),
            missing_from_backup: None,
            extra_only_in_backup: None,
            explanation: "backup objects are readable, but primary failure prevents an RPO or completeness comparison".to_string(),
        };
    }

    let mut missing = usize::default();
    let mut extra = usize::default();
    for prefix in CANONICAL_PREFIXES {
        let primary_names = primary.names.get(*prefix).cloned().unwrap_or_default();
        let backup_names = backup.names.get(*prefix).cloned().unwrap_or_default();
        missing = missing.saturating_add(primary_names.difference(&backup_names).count());
        extra = extra.saturating_add(backup_names.difference(&primary_names).count());
    }
    let state = if missing == usize::default() {
        "namespace_covered"
    } else {
        "incomplete"
    };
    CoverageReport {
        state: state.to_string(),
        missing_from_backup: Some(missing),
        extra_only_in_backup: Some(extra),
        explanation: "coverage compares canonical object names; content and metadata integrity remain the responsibility of storage copy verify".to_string(),
    }
}

fn data_domains(primary: &StorageInspection) -> Vec<DomainReport> {
    vec![
        domain(
            "job_lifecycle",
            JOB_LIFECYCLE,
            &[
                "scheduler",
                "workers",
                "status",
                "results",
                "cancel",
                "desktop and API job views",
            ],
            primary,
        ),
        domain(
            "scheduler_control",
            SCHEDULER_CONTROL,
            &[
                "coordinator",
                "scheduler",
                "quota reservations",
                "recurring jobs",
            ],
            primary,
        ),
        domain(
            "fleet_observability",
            FLEET_OBSERVABILITY,
            &["dashboard", "overview", "host health", "billing health"],
            primary,
        ),
        domain(
            "automation_requests",
            AUTOMATION,
            &["machine API", "automation clients"],
            primary,
        ),
        domain(
            "runs_artifacts_and_scripts",
            PAYLOADS,
            &[
                "run history",
                "artifact consumers",
                "worker startup scripts",
            ],
            primary,
        ),
        domain(
            "registry_and_secrets",
            CONTROL_AND_SECRETS,
            &[
                "coordinators",
                "host commands",
                "billing collectors",
                "provider credentials",
            ],
            primary,
        ),
    ]
}

fn domain(
    name: &str,
    prefixes: &[&str],
    consumers: &[&str],
    storage: &StorageInspection,
) -> DomainReport {
    let count = if storage.report.state == "reachable" {
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
        consumers: consumers.iter().map(|value| (*value).to_string()).collect(),
    }
}

fn downstream_impacts(
    dependency: &str,
    primary: &StorageReport,
    backup: &StorageReport,
) -> Vec<DownstreamImpact> {
    let storage_hit = dependency_owns_backend(dependency, config::wc_storage_backend());
    let primary_down = primary.state != "reachable";
    let backup_readable = backup.state == "reachable";
    let storage_state = if !storage_hit {
        "unaffected"
    } else if !primary_down {
        "at_risk"
    } else if backup_readable {
        "blocked_backup_requires_explicit_promotion"
    } else {
        "blocked_no_readable_backup"
    };

    let mut impacts =
        vec![
        impact(
            "queue_store",
            "critical",
            storage_state,
            CANONICAL_PREFIXES,
            &[
                "coordinator",
                "scheduler",
                "workers",
                "dashboard",
                "desktop",
                "status/results/cancel",
                "machine API",
            ],
            "the mutable queue and every lifecycle transition use the configured storage backend",
        ),
        impact(
            "registry_and_secrets",
            "critical",
            storage_state,
            CONTROL_AND_SECRETS,
            &["coordinators", "host management", "provider and billing authentication"],
            "registry.json and Stado-managed secrets live in the same primary store",
        ),
    ];

    let provider_enabled = config::wc_providers()
        .iter()
        .any(|provider| provider == dependency);
    impacts.push(impact(
        "compute_provider",
        "critical",
        if provider_enabled {
            "blocked_or_degraded"
        } else {
            "unaffected"
        },
        &[],
        &[
            "scheduler dispatch",
            "ephemeral GPU workers",
            "quota inspection",
        ],
        "configured compute providers are the scheduler's VM creation and lifecycle surface",
    ));

    let release_hit = dependency_owns_release(dependency, config::release_base_url());
    impacts.push(impact(
        "release_channel",
        "high",
        if release_hit { "blocked" } else { "unaffected" },
        &["releases/stado/"],
        &[
            "self update",
            "bootstrap",
            "new cloud workers",
            "host repair",
        ],
        "new processes need the binary and checksum channel even when existing processes still run",
    ));

    let pubsub_hit = dependency == "gcp" && !config::alerts_topic().is_empty();
    impacts.push(impact(
        "pubsub_alert_sink",
        "medium",
        if pubsub_hit { "degraded" } else { "unaffected" },
        &[],
        &["incident notifications"],
        "Pub/Sub is one alert sink; independently configured Slack, Telegram or SendGrid sinks can survive",
    ));

    impacts.push(impact(
        "provider_billing_visibility",
        "high",
        if dependency == "local" {
            "unaffected"
        } else {
            "degraded"
        },
        &["billing_health/"],
        &["overview", "billing watch", "credit depletion alerts"],
        "provider account and billing APIs become unreadable with the failed provider",
    ));

    impacts.push(impact(
        "supabase_deployment_metadata",
        "low",
        "unaffected",
        &[],
        &[
            "deployment list",
            "deployment grants",
            "infrastructure target metadata",
        ],
        "Supabase is outside the queue store and cloud-provider project",
    ));

    impacts
}

fn impact(
    component: &str,
    severity: &str,
    state: &str,
    data: &[&str],
    consumers: &[&str],
    reason: &str,
) -> DownstreamImpact {
    DownstreamImpact {
        component: component.to_string(),
        severity: severity.to_string(),
        state: state.to_string(),
        data: data.iter().map(|value| (*value).to_string()).collect(),
        consumers: consumers.iter().map(|value| (*value).to_string()).collect(),
        reason: reason.to_string(),
    }
}

fn dependency_owns_backend(dependency: &str, backend: &str) -> bool {
    matches!(
        (dependency, backend),
        ("gcp", "gcs") | ("azure", "azure") | ("aws", "s3") | ("local", "local")
    )
}

fn dependency_owns_release(dependency: &str, url: &str) -> bool {
    match dependency {
        "gcp" => url.contains("googleapis.com") || url.contains("storage.cloud.google.com"),
        "azure" => url.contains("blob.core.windows.net"),
        "aws" => url.contains("amazonaws.com"),
        "local" => url.starts_with("file:"),
        _ => false,
    }
}

fn print_human(report: &BlastRadiusReport) {
    println!("Dependency: {}", report.dependency);
    println!("Overall: {}", report.summary.state);
    println!(
        "Primary: {} ({})",
        report
            .primary_storage
            .locator
            .as_deref()
            .unwrap_or("not configured"),
        report.primary_storage.state
    );
    println!(
        "Backup: {} ({})",
        report
            .backup_storage
            .locator
            .as_deref()
            .unwrap_or("not configured"),
        report.backup_storage.state
    );
    println!("Backup coverage: {}", report.backup_coverage.state);
    println!("Scale source: {}", report.summary.scale_source);
    println!(
        "Objects in scope: primary={}, backup={}",
        optional_count(report.summary.primary_objects_in_scope),
        optional_count(report.summary.backup_objects_in_scope)
    );
    println!();
    println!("Downstream consumers:");
    for impact in &report.downstream {
        println!(
            "- [{}] {}: {} — {}",
            impact.severity,
            impact.component,
            impact.state,
            impact.consumers.join(", ")
        );
    }
    println!();
    println!("Automatic failover: disabled");
    println!("Safe mode: {}", report.failover.safe_mode);
    if let Some(error) = &report.primary_storage.error {
        println!("Primary error: {error}");
    }
    if let Some(error) = &report.backup_storage.error {
        println!("Backup error: {error}");
    }
    println!(
        "Machine-readable detail: stado blast-radius --dependency {} --json",
        report.dependency
    );
    println!("Recovery order:");
    for step in &report.recovery_order {
        println!("- {step}");
    }
}

fn optional_count(value: Option<usize>) -> String {
    value.map_or_else(|| "unknown".to_string(), |count| count.to_string())
}
