//! `stado blast-radius` — side-effect-free incident scope and DR readiness.
//!
//! The command keeps failure domains separate instead of collapsing them into
//! "the queue is empty": primary and backup stores, Skarbiec credentials,
//! live cloud resources and caller/runtime IAM, downstream consumers, and
//! backup namespace coverage. Provider probes are independent and paginated,
//! so one disabled API cannot hide the remaining project inventory.
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
use serde_json::Value;

use crate::providers::gcp::inventory::{
    self as gcp_inventory, GcpInventoryReport, InventoryOptions,
};
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
struct CredentialStoreReport {
    state: String,
    locator: String,
    consumer: String,
    item_count: Option<usize>,
    items: Vec<crate::skarbiec::ItemInfo>,
    missing_required: Vec<String>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct Summary {
    state: String,
    affected_components: usize,
    primary_objects_in_scope: Option<usize>,
    backup_objects_in_scope: Option<usize>,
    scale_source: String,
    infrastructure_state: Option<String>,
    infrastructure_checks: usize,
    infrastructure_failures: usize,
    credential_store_state: String,
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
    infrastructure: Option<GcpInventoryReport>,
    credential_store: CredentialStoreReport,
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
const REGISTRY: &[&str] = &["registry.json"];

pub async fn run(args: &BlastRadiusArgs) -> Result<(), CmdError> {
    let dependency = validate_dependency(&args.dependency)?;

    let primary_endpoint = Endpoint::configured_primary();
    let backup_endpoint = Endpoint::configured_backup();
    let inventory_options = gcp_inventory_options(&primary_endpoint, backup_endpoint.as_ref());
    let inventory_probe = async {
        if matches!(
            dependency.adapter,
            crate::capabilities::RuntimeAdapter::Dependency(
                crate::capabilities::DependencyAdapter::Gcp
            )
        ) {
            Some(gcp_inventory::inspect(inventory_options).await)
        } else {
            None
        }
    };
    let (primary, backup, infrastructure, credential_store) = tokio::join!(
        inspect_storage_bounded("primary", Some(&primary_endpoint)),
        inspect_storage_bounded("backup", backup_endpoint.as_ref()),
        inventory_probe,
        inspect_credential_store(),
    );
    let backup_matches_primary = backup_endpoint
        .as_ref()
        .is_some_and(|endpoint| endpoint.describe() == primary_endpoint.describe());
    let coverage = compare_coverage(
        &primary,
        &backup,
        backup_endpoint.is_some(),
        backup_matches_primary,
    );
    let domains = data_domains(&primary);
    let downstream = downstream_impacts(dependency, &primary.report, &backup.report);
    let affected_components = downstream
        .iter()
        .filter(|impact| impact.state != "unaffected")
        .count();
    let primary_unavailable = primary.report.state != "reachable";
    let dependency_owns_primary = dependency_owns_backend(dependency, config::wc_storage_backend());
    let infrastructure_critical = infrastructure
        .as_ref()
        .is_some_and(|report| report.summary.critical_failures != usize::default());
    let credential_store_critical = credential_store.state != "reachable";
    let state = if infrastructure_critical
        || credential_store_critical
        || (dependency_owns_primary && primary_unavailable)
    {
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

    let infrastructure_state = infrastructure
        .as_ref()
        .map(|report| report.summary.state.clone());
    let infrastructure_checks = infrastructure
        .as_ref()
        .map_or(usize::default(), |report| report.summary.probes);
    let infrastructure_failures = infrastructure.as_ref().map_or(usize::default(), |report| {
        report
            .probes
            .iter()
            .filter(|probe| probe.state != "ok")
            .count()
    });
    let credential_store_state = credential_store.state.clone();

    let report = BlastRadiusReport {
        dependency: dependency.id.to_string(),
        configured_storage_backend: config::wc_storage_backend().to_string(),
        configured_compute_providers: config::wc_providers().to_vec(),
        summary: Summary {
            state: state.to_string(),
            affected_components,
            primary_objects_in_scope: primary_scope,
            backup_objects_in_scope: backup_scope,
            scale_source,
            infrastructure_state,
            infrastructure_checks,
            infrastructure_failures,
            credential_store_state,
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
        infrastructure,
        credential_store,
        recovery_order: [
            "establish one readable authoritative store",
            "fence every scheduler and worker writer",
            "verify backup namespace and metadata",
            "promote the selected backend to every participant",
            "restore registry and release distribution",
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

async fn inspect_credential_store() -> CredentialStoreReport {
    const REQUIRED_ITEMS: &[&str] = &["stado-huggingface"];
    let locator = crate::credential_store::requested_selector()
        .unwrap_or_else(|error| format!("invalid selector: {error}"));
    let credentials = match crate::credential_store::admin_credentials() {
        Ok(credentials) => credentials,
        Err(error) => {
            return CredentialStoreReport {
                state: "unreachable".to_string(),
                locator,
                consumer: String::new(),
                item_count: None,
                items: Vec::new(),
                missing_required: REQUIRED_ITEMS
                    .iter()
                    .map(|item| (*item).to_string())
                    .collect(),
                error: Some(error.to_string()),
            }
        }
    };
    let consumer = credentials.consumer.clone();
    let client = match crate::skarbiec::Client::new(
        &credentials.url,
        &credentials.consumer,
        &credentials.token_file,
    ) {
        Ok(client) => client,
        Err(error) => {
            return CredentialStoreReport {
                state: "unreachable".to_string(),
                locator,
                consumer,
                item_count: None,
                items: Vec::new(),
                missing_required: REQUIRED_ITEMS
                    .iter()
                    .map(|item| (*item).to_string())
                    .collect(),
                error: Some(error.to_string()),
            }
        }
    };
    let listed = match tokio::time::timeout(crate::doctor::PROBE_TIMEOUT, client.list_items()).await
    {
        Ok(result) => result.map_err(|error| error.to_string()),
        Err(_) => Err(format!(
            "credential store inspection exceeded {:?}",
            crate::doctor::PROBE_TIMEOUT
        )),
    };
    match listed {
        Ok(mut items) => {
            items.retain(|item| item.deleted != Some(true));
            items.sort_by(|left, right| left.id.cmp(&right.id));
            let present: BTreeSet<&str> = items.iter().map(|item| item.id.as_str()).collect();
            let missing_required: Vec<String> = REQUIRED_ITEMS
                .iter()
                .copied()
                .filter(|item| !present.contains(item))
                .map(str::to_string)
                .collect();
            CredentialStoreReport {
                state: if missing_required.is_empty() {
                    "reachable"
                } else {
                    "degraded"
                }
                .to_string(),
                locator,
                consumer,
                item_count: Some(items.len()),
                items,
                missing_required,
                error: None,
            }
        }
        Err(error) => CredentialStoreReport {
            state: "unreachable".to_string(),
            locator,
            consumer,
            item_count: None,
            items: Vec::new(),
            missing_required: REQUIRED_ITEMS
                .iter()
                .map(|item| (*item).to_string())
                .collect(),
            error: Some(error.to_string()),
        },
    }
}

pub(crate) fn gcp_inventory_options(
    primary: &Endpoint,
    backup: Option<&Endpoint>,
) -> InventoryOptions {
    let mut buckets = BTreeSet::new();
    for endpoint in [Some(primary), backup].into_iter().flatten() {
        if endpoint.adapter() == Some(crate::capabilities::StorageAdapter::Gcs)
            && !endpoint.bucket.is_empty()
        {
            buckets.insert(endpoint.bucket.clone());
        }
    }

    InventoryOptions {
        project: config::project().to_string(),
        region: config::region().to_string(),
        regions: config::regions().to_vec(),
        buckets: buckets.into_iter().collect(),
        objects: Vec::new(),
        alerts_topic: config::alerts_topic().to_string(),
        billing_dataset: config::billing_dataset().to_string(),
        billing_table: config::billing_table().to_string(),
    }
}

fn validate_dependency(
    dependency: &str,
) -> Result<&'static crate::capabilities::CapabilityVariant, CmdError> {
    if let Some(variant) = crate::capabilities::configurable_variant(
        crate::capabilities::RuntimeFacet::Dependency,
        dependency,
    ) {
        return Ok(variant);
    }
    let choices =
        crate::capabilities::configurable_ids(crate::capabilities::RuntimeFacet::Dependency)
            .collect::<Vec<_>>()
            .join(", ");
    Err(CmdError::click(format!(
        "unknown dependency {dependency:?}; use one of: {choices}"
    )))
}

async fn inspect_storage_bounded(role: &str, endpoint: Option<&Endpoint>) -> StorageInspection {
    match tokio::time::timeout(
        crate::doctor::PROBE_TIMEOUT,
        inspect_storage(role, endpoint),
    )
    .await
    {
        Ok(inspection) => inspection,
        Err(_) => StorageInspection {
            report: StorageReport {
                role: role.to_string(),
                locator: endpoint.map(Endpoint::describe),
                state: "unreachable".to_string(),
                object_count: None,
                newest_object_at: None,
                error: Some(format!(
                    "storage inspection exceeded {:?}",
                    crate::doctor::PROBE_TIMEOUT
                )),
                prefixes: Vec::new(),
            },
            names: BTreeMap::new(),
        },
    }
}

/// Provider-neutral storage projection used by `resources show`.
pub(crate) async fn storage_resource_report(role: &str, endpoint: Option<&Endpoint>) -> Value {
    serde_json::to_value(inspect_storage_bounded(role, endpoint).await.report)
        .expect("storage report serialization is infallible")
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
            "registry",
            REGISTRY,
            &["coordinators", "host commands"],
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
    dependency: &crate::capabilities::CapabilityVariant,
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

    let mut impacts = vec![
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
            "registry",
            "critical",
            storage_state,
            REGISTRY,
            &["coordinators", "host management"],
            "registry.json lives in the configured primary store; credentials live in the globally selected credential store",
        ),
    ];

    let provider_enabled = dependency.provider.is_some_and(|owner| {
        config::wc_providers()
            .iter()
            .any(|provider| owner.matches(provider))
    });
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

    let release_hit = dependency_owns_release(dependency, &config::stado_release_api_url());
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

    let pubsub_hit = dependency.provider == Some(crate::capabilities::ProviderId::Gcp)
        && !config::alerts_topic().is_empty();
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
        if dependency.provider == Some(crate::capabilities::ProviderId::Local) {
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

fn dependency_owns_backend(
    dependency: &crate::capabilities::CapabilityVariant,
    backend: &str,
) -> bool {
    let backend_owner =
        crate::capabilities::variant(crate::capabilities::RuntimeFacet::Storage, backend)
            .and_then(|variant| variant.provider);
    dependency.provider.is_some() && dependency.provider == backend_owner
}

fn dependency_owns_release(dependency: &crate::capabilities::CapabilityVariant, url: &str) -> bool {
    dependency
        .provider
        .is_some_and(|provider| provider.owns_release_url(url))
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
    println!(
        "Credential store: {} ({}, consumer={}, items={})",
        report.credential_store.state,
        report.credential_store.locator,
        report.credential_store.consumer,
        optional_count(report.credential_store.item_count),
    );
    if !report.credential_store.items.is_empty() {
        println!(
            "  item ids: {}",
            report
                .credential_store
                .items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if !report.credential_store.missing_required.is_empty() {
        println!(
            "  missing required items: {}",
            report.credential_store.missing_required.join(", ")
        );
    }
    if let Some(error) = &report.credential_store.error {
        println!("  error: {}", error.lines().next().unwrap_or(error));
    }
    if let Some(infrastructure) = &report.infrastructure {
        println!();
        println!(
            "GCP infrastructure: {} (checks={}, healthy={}, critical_failures={})",
            infrastructure.summary.state,
            infrastructure.summary.probes,
            infrastructure.summary.healthy,
            infrastructure.summary.critical_failures,
        );
        for probe in &infrastructure.probes {
            let count = probe
                .count
                .map_or_else(String::new, |count| format!(", count={count}"));
            println!(
                "- [{}] {} / {}: {}{} — {}",
                probe.severity, probe.service, probe.name, probe.state, count, probe.resource,
            );
            if let Some(error) = &probe.error {
                println!("  error: {}", error.lines().next().unwrap_or(error));
            }
            print_probe_highlights(probe);
        }
    }

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

fn print_probe_highlights(probe: &crate::providers::gcp::inventory::ProbeReport) {
    match probe.name.as_str() {
        "billing_account" => {
            if let Some(enabled) = probe.detail.get("billing_enabled") {
                println!("  billing_enabled: {enabled}");
            }
        }
        "caller_permissions" => {
            if let Some(missing) = probe
                .detail
                .get("missing")
                .and_then(serde_json::Value::as_array)
            {
                if !missing.is_empty() {
                    println!(
                        "  missing permissions: {}",
                        missing
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
            }
        }
        "service_account_roles" => {
            if let Some(missing) = probe.detail.get("missing_by_service_account") {
                println!("  missing runtime roles by account: {missing}");
            }
        }
        "compute_instances" => {
            if let Some(statuses) = probe.detail.get("by_status") {
                println!("  statuses: {statuses}");
            }
            if let Some(instances) = probe
                .detail
                .get("instances")
                .and_then(serde_json::Value::as_array)
            {
                for instance in instances.iter().filter(|instance| {
                    instance
                        .get("status")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|status| {
                            matches!(
                                status,
                                "RUNNING"
                                    | "STAGING"
                                    | "PROVISIONING"
                                    | "REPAIRING"
                                    | "STOPPING"
                                    | "SUSPENDING"
                            )
                        })
                }) {
                    println!(
                        "  active VM: name={}, zone={}, status={}, machine={}, accelerators={}",
                        instance.get("name").unwrap_or(&serde_json::Value::Null),
                        instance.get("zone").unwrap_or(&serde_json::Value::Null),
                        instance.get("status").unwrap_or(&serde_json::Value::Null),
                        instance
                            .get("machine_type")
                            .unwrap_or(&serde_json::Value::Null),
                        instance
                            .get("accelerators")
                            .unwrap_or(&serde_json::Value::Null),
                    );
                }
            }
        }
        "compute_disks" => {
            println!(
                "  disk_gb={}, unattached={}",
                probe
                    .detail
                    .get("total_gb")
                    .unwrap_or(&serde_json::Value::Null),
                probe
                    .detail
                    .get("unattached")
                    .unwrap_or(&serde_json::Value::Null),
            );
        }
        "managed_instance_groups" => {
            if let Some(target) = probe.detail.get("target_instances") {
                println!("  desired instances across MIGs: {target}");
            }
        }
        "cloud_run_service" => {
            if let Some(revision) = probe.detail.get("latest_ready_revision") {
                println!("  latest ready revision: {revision}");
            }
            if let Some(environment) = probe.detail.get("environment") {
                println!("  non-secret runtime config: {environment}");
            }
        }
        _ if probe.name.starts_with("compute_region_quota_") => {
            if let Some(exhausted) = probe.detail.get("exhausted") {
                println!("  exhausted quota: {exhausted}");
            }
        }
        _ => {}
    }
}

fn optional_count(value: Option<usize>) -> String {
    value.map_or_else(|| "unknown".to_string(), |count| count.to_string())
}
