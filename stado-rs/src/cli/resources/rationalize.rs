//! `stado resources rationalize` — read-only audit and immutable plan creation.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::Digest;

use super::journal::Journal;
use super::model::{
    Action, ActionKind, Authorization, Finding as PlanFinding, FindingDisposition, Intent,
    InventorySnapshot, OperationScope, ProviderKind, ResourceLocator, Reversibility, Rollback,
    SourceSnapshot,
};
use super::{planner, RationalizeArgs};
use crate::cli::{blast_radius, instances, table, CmdError};
use crate::config;
use crate::providers::gcp::inventory::{self as gcp_inventory, GcpInventoryReport, ProbeReport};
use crate::queue::copy::Endpoint;
use crate::queue::JobStorage;

struct AuditArgs {
    min_age: u64,
}

#[derive(Debug, Clone, Serialize)]
struct Finding {
    id: String,
    severity: &'static str,
    action: &'static str,
    confidence: &'static str,
    provider: String,
    resource_type: &'static str,
    resource: String,
    reason: String,
    evidence: Value,
    automatic: bool,
}

#[derive(Debug, Clone, Serialize)]
struct SourceReport {
    name: String,
    state: &'static str,
    detail: Value,
}

#[derive(Debug, Clone, Serialize)]
struct ConfigurationSnapshot {
    active_compute: Vec<String>,
    disabled_compute: Vec<String>,
    primary_storage: String,
    backup_storage: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct Summary {
    state: &'static str,
    findings: usize,
    incomplete_sources: usize,
    by_severity: BTreeMap<String, usize>,
    by_action: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Serialize)]
struct RationalizationReport {
    schema_version: u8,
    generated_at: String,
    read_only: bool,
    min_age_seconds: u64,
    configuration: ConfigurationSnapshot,
    summary: Summary,
    sources: Vec<SourceReport>,
    findings: Vec<Finding>,
}

pub async fn run(args: &RationalizeArgs) -> Result<(), CmdError> {
    let age = planner::parse_age(&args.min_age)?;
    let min_age = u64::try_from(age.num_seconds())
        .map_err(|_| CmdError::usage("--min-age is outside the supported range"))?;
    let report = build_report(&AuditArgs { min_age }).await?;
    let plan = compile_plan(&report, args.provider.as_deref())?;
    let hash = planner::write_plan(&plan, &args.output)?;
    Journal::open().await?.create(&plan).await?;
    if !args.json {
        print_human(&report);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "operation_id": plan.operation_id,
            "plan": args.output,
            "sha256": hash,
            "findings": plan.findings.len(),
            "actions": plan.actions.len(),
            "automatic_actions": plan.actions.iter().filter(|action| action.authorization == Authorization::Automatic).count(),
            "review_required": plan.actions.iter().filter(|action| action.authorization == Authorization::Explicit).count(),
        }))?
    );
    Ok(())
}

async fn build_report(args: &AuditArgs) -> Result<RationalizationReport, CmdError> {
    let primary = Endpoint::configured_primary();
    let backup = Endpoint::configured_backup();
    let active = config::wc_providers().to_vec();
    let disabled = config::wc_disabled_providers().to_vec();
    let configured: BTreeSet<String> = active.iter().chain(&disabled).cloned().collect();
    let now = Utc::now();

    let mut findings = configuration_findings(&primary, backup.as_ref(), &active, &disabled);
    let mut sources = vec![SourceReport {
        name: "stado-configuration".to_string(),
        state: "ok",
        detail: json!({
            "active_compute": active,
            "disabled_compute": disabled,
            "primary_storage": primary.describe(),
            "backup_storage": backup.as_ref().map(Endpoint::describe),
        }),
    }];

    let fleet_providers: Vec<String> =
        crate::capabilities::provider_ids(crate::capabilities::RuntimeFacet::Inventory)
            .into_iter()
            .map(|provider| provider.as_str().to_string())
            .filter(|provider| configured.contains(provider))
            .collect();
    if fleet_providers.is_empty() {
        sources.push(SourceReport {
            name: "agent-vm-ownership".to_string(),
            state: "skipped",
            detail: json!({"reason": "no enumerable cloud or marketplace compute provider is configured"}),
        });
    } else if primary.adapter() == Some(crate::capabilities::StorageAdapter::Local) {
        sources.push(SourceReport {
            name: "agent-vm-ownership".to_string(),
            state: "blocked",
            detail: json!({
                "reason": "device-local storage is not an authoritative ownership view for remote cloud agents",
                "remedy": "migrate the active queue to GCS, S3, or Azure Blob before using orphan VM recommendations",
            }),
        });
    } else {
        match JobStorage::new().await {
            Ok(store) => match instances::audit_inventory(&store, &fleet_providers).await {
                Ok(fleet) => {
                    for provider in &fleet_providers {
                        if let Some(error) = fleet.errors.get(provider) {
                            sources.push(SourceReport {
                                name: format!("{provider}-agent-vm-ownership"),
                                state: "blocked",
                                detail: json!({"error": error}),
                            });
                        } else {
                            let count = fleet
                                .rows
                                .iter()
                                .filter(|row| row.provider == *provider)
                                .count();
                            sources.push(SourceReport {
                                name: format!("{provider}-agent-vm-ownership"),
                                state: "ok",
                                detail: json!({"instances": count}),
                            });
                        }
                    }
                    findings.extend(orphan_instance_findings(&fleet.rows, args.min_age));
                }
                Err(error) => sources.push(SourceReport {
                    name: "agent-vm-ownership".to_string(),
                    state: "blocked",
                    detail: json!({"error": error.to_string()}),
                }),
            },
            Err(error) => sources.push(SourceReport {
                name: "agent-vm-ownership".to_string(),
                state: "blocked",
                detail: json!({
                    "error": error.to_string(),
                    "reason": "the authoritative queue and lease store could not be opened",
                }),
            }),
        }
    }

    for variant in crate::capabilities::get("inventory")
        .into_iter()
        .flat_map(|capability| capability.variants)
    {
        let Some(provider) = variant.provider else {
            continue;
        };
        let provider_configured = configured.iter().any(|name| provider.matches(name));
        match variant.adapter {
            crate::capabilities::RuntimeAdapter::Inventory(
                crate::capabilities::InventoryAdapter::Gcp,
            ) if provider_configured => {
                let options = blast_radius::gcp_inventory_options(&primary, backup.as_ref());
                let report = gcp_inventory::inspect(options).await;
                sources.push(SourceReport {
                    name: format!("{}-resource-inventory", variant.id),
                    state: if report.summary.state == "ok" {
                        "ok"
                    } else {
                        "degraded"
                    },
                    detail: json!({
                        "project": report.project,
                        "summary": report.summary,
                    }),
                });
                findings.extend(gcp_findings(
                    &report,
                    args.min_age,
                    now,
                    disabled.iter().any(|name| provider.matches(name)),
                ));
            }
            crate::capabilities::RuntimeAdapter::Inventory(
                crate::capabilities::InventoryAdapter::Gcp,
            ) => sources.push(SourceReport {
                name: format!("{}-resource-inventory", variant.id),
                state: "skipped",
                detail: json!({"reason": format!("{} is absent from providers and providers_disabled", provider)}),
            }),
            crate::capabilities::RuntimeAdapter::Inventory(_) if provider_configured => {
                sources.push(SourceReport {
                    name: format!("{}-resource-inventory", variant.id),
                    state: "unsupported",
                    detail: json!({
                        "reason": provider.inventory_limitation().unwrap_or(variant.summary),
                        "remedy": format!("review the {provider} provider console before accepting this report as complete"),
                    }),
                });
            }
            _ => {}
        }
    }

    findings.sort_by(|left, right| {
        severity_rank(left.severity)
            .cmp(&severity_rank(right.severity))
            .then_with(|| left.provider.cmp(&right.provider))
            .then_with(|| left.resource.cmp(&right.resource))
    });
    let incomplete_sources = sources
        .iter()
        .filter(|source| matches!(source.state, "blocked" | "degraded" | "unsupported"))
        .count();
    let summary = summarize(&findings, incomplete_sources);
    let report = RationalizationReport {
        schema_version: u8::from(true),
        generated_at: now.to_rfc3339_opts(SecondsFormat::Secs, true),
        read_only: true,
        min_age_seconds: args.min_age,
        configuration: ConfigurationSnapshot {
            active_compute: active,
            disabled_compute: disabled,
            primary_storage: primary.describe(),
            backup_storage: backup.as_ref().map(Endpoint::describe),
        },
        summary,
        sources,
        findings,
    };

    Ok(report)
}

fn compile_plan(
    report: &RationalizationReport,
    selected_provider: Option<&str>,
) -> Result<super::model::Plan, CmdError> {
    let selected_provider = selected_provider.map(str::trim);
    let selected: Vec<&Finding> = report
        .findings
        .iter()
        .filter(|finding| selected_provider.is_none_or(|provider| finding.provider == provider))
        .collect();
    if let Some(provider) = selected_provider {
        let configured = config::wc_providers()
            .iter()
            .chain(config::wc_disabled_providers())
            .any(|name| name == provider);
        let reported = selected.iter().any(|finding| finding.provider == provider);
        if !configured && !reported {
            return Err(CmdError::usage(format!(
                "provider {provider:?} is neither configured nor present in the audit"
            )));
        }
    }

    let findings: Vec<PlanFinding> = selected
        .iter()
        .map(|finding| PlanFinding {
            id: finding.id.clone(),
            severity: finding.severity.to_string(),
            confidence: finding.confidence.to_string(),
            recommendation: finding.action.to_string(),
            reason: finding.reason.clone(),
            evidence: finding.evidence.clone(),
            disposition: finding_disposition(finding),
            resource: locator(finding),
        })
        .collect();
    let mut actions = Vec::new();
    for finding in &selected {
        actions.extend(actions_for(finding));
    }
    let providers = findings
        .iter()
        .map(|finding| finding.resource.provider)
        .collect();
    let mut projects = BTreeSet::new();
    if findings
        .iter()
        .any(|finding| finding.resource.provider == ProviderKind::Gcp)
        && !config::project().is_empty()
    {
        projects.insert(config::project().to_string());
    }
    let report_value = serde_json::to_value(report)?;
    let snapshot_id = hex::encode(sha2::Sha256::digest(serde_json::to_vec(&report_value)?));
    let inventory = InventorySnapshot {
        snapshot_id,
        complete: report.summary.incomplete_sources == usize::default(),
        sources: report
            .sources
            .iter()
            .map(|source| SourceSnapshot {
                name: source.name.clone(),
                state: source.state.to_string(),
                detail: source.detail.clone(),
            })
            .collect(),
    };
    planner::new_plan(
        Intent::RationalizationCleanup,
        OperationScope {
            providers,
            projects,
            storage: Endpoint::configured_primary().describe(),
        },
        inventory,
        findings,
        actions,
    )
}

fn finding_disposition(finding: &Finding) -> FindingDisposition {
    if finding.automatic {
        FindingDisposition::Automatic
    } else if matches!(finding.action, "disable-or-migrate" | "review-deprovision") {
        FindingDisposition::Blocked
    } else {
        FindingDisposition::ReviewRequired
    }
}

fn actions_for(finding: &Finding) -> Vec<Action> {
    let resource_scope = gcp_scope(finding);
    let resource = locator(finding);
    let authorization = if finding.automatic {
        Authorization::Automatic
    } else {
        Authorization::Explicit
    };
    let action_id = format!("action-{}", uuid::Uuid::new_v4().simple());
    match finding.resource_type {
        "agent-vm" => vec![Action {
            id: action_id,
            finding_id: Some(finding.id.clone()),
            kind: ActionKind::DeleteInstance,
            authorization,
            reversibility: Reversibility::Irreversible,
            resource,
            parameters: json!({}),
            preconditions: vec![
                planner::condition("orphan", json!(true)),
                planner::condition(
                    "minimum_age_seconds",
                    finding.evidence["age_seconds"].clone(),
                ),
            ],
            postconditions: vec![planner::condition("exists", json!(false))],
            rollback: None,
            depends_on: Vec::new(),
        }],
        "persistent-disk" => {
            let snapshot_id = format!("action-{}", uuid::Uuid::new_v4().simple());
            let snapshot_name = recovery_snapshot_name(&resource.name);
            vec![
                Action {
                    id: snapshot_id.clone(),
                    finding_id: Some(finding.id.clone()),
                    kind: ActionKind::SnapshotDisk,
                    authorization: Authorization::Explicit,
                    reversibility: Reversibility::Reversible,
                    resource: resource.clone(),
                    parameters: json!({"snapshot_name": snapshot_name, "scope": resource_scope}),
                    preconditions: stable_preconditions(
                        finding,
                        vec![planner::condition("unattached", json!(true))],
                    ),
                    postconditions: vec![planner::condition("snapshot_exists", json!(true))],
                    rollback: Some(Rollback {
                        kind: ActionKind::DeleteSnapshot,
                        parameters: json!({"snapshot_name": snapshot_name}),
                        preconditions: vec![planner::condition("snapshot_exists", json!(true))],
                        postconditions: vec![planner::condition("snapshot_exists", json!(false))],
                    }),
                    depends_on: Vec::new(),
                },
                Action {
                    id: action_id,
                    finding_id: Some(finding.id.clone()),
                    kind: ActionKind::DeleteDisk,
                    authorization: Authorization::Explicit,
                    reversibility: Reversibility::SnapshotRestore,
                    resource,
                    parameters: json!({"snapshot_name": snapshot_name, "scope": resource_scope, "original": finding.evidence}),
                    preconditions: stable_preconditions(
                        finding,
                        vec![planner::condition("unattached", json!(true))],
                    ),
                    postconditions: vec![planner::condition("exists", json!(false))],
                    rollback: Some(Rollback {
                        kind: ActionKind::RestoreDisk,
                        parameters: json!({"snapshot_name": snapshot_name, "scope": resource_scope, "original": finding.evidence}),
                        preconditions: vec![
                            planner::condition("exists", json!(false)),
                            planner::condition("snapshot_exists", json!(true)),
                        ],
                        postconditions: disk_restore_postconditions(finding, &snapshot_name),
                    }),
                    depends_on: vec![snapshot_id],
                },
            ]
        }
        "static-address" => vec![irreversible_action(
            action_id,
            finding,
            ActionKind::ReleaseAddress,
            resource,
            json!({"scope": resource_scope}),
            stable_preconditions(
                finding,
                vec![
                    planner::condition("unused", json!(true)),
                    planner::condition("status", json!("RESERVED")),
                ],
            ),
        )],
        "managed-instance-group" => vec![irreversible_action(
            action_id,
            finding,
            ActionKind::DeleteManagedInstanceGroup,
            resource,
            json!({"scope": resource_scope}),
            stable_preconditions(
                finding,
                vec![planner::condition("target_size", json!(usize::default()))],
            ),
        )],
        "compute-reservation" => vec![irreversible_action(
            action_id,
            finding,
            ActionKind::ReleaseReservation,
            resource,
            json!({"scope": resource_scope}),
            stable_preconditions(
                finding,
                vec![
                    planner::condition("exists", json!(true)),
                    planner::condition("in_use_count", json!(usize::default())),
                ],
            ),
        )],
        "storage-backup"
            if finding.action == "disable" && finding.evidence["backup_config"].is_object() =>
        {
            vec![Action {
                id: action_id,
                finding_id: Some(finding.id.clone()),
                kind: ActionKind::DisableStorageBackup,
                authorization: Authorization::Explicit,
                reversibility: Reversibility::Reversible,
                resource,
                parameters: json!({"previous": finding.evidence["backup_config"]}),
                preconditions: vec![
                    planner::condition("configured", json!(true)),
                    planner::condition("mutable", json!(true)),
                    planner::condition("backup", finding.evidence["backup_config"].clone()),
                ],
                postconditions: vec![planner::condition("configured", json!(false))],
                rollback: Some(Rollback {
                    kind: ActionKind::EnableStorageBackup,
                    parameters: json!({"backup": finding.evidence["backup_config"]}),
                    preconditions: vec![planner::condition("configured", json!(false))],
                    postconditions: vec![
                        planner::condition("configured", json!(true)),
                        planner::condition("backup", finding.evidence["backup_config"].clone()),
                    ],
                }),
                depends_on: Vec::new(),
            }]
        }
        _ => Vec::new(),
    }
}
fn gcp_scope(finding: &Finding) -> &'static str {
    let region = finding
        .evidence
        .get("region")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if finding.resource_type == "static-address" && region.is_empty() {
        "global"
    } else if !region.is_empty() {
        "region"
    } else {
        "zone"
    }
}

fn recovery_snapshot_name(disk_name: &str) -> String {
    let prefix = "stado-recovery-";
    let nonce: String = uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take((u64::BITS / u8::BITS) as usize)
        .collect();
    let suffix = format!("-{}-{nonce}", Utc::now().format("%Y%m%d%H%M%S"));
    let maximum = (u64::BITS as usize).saturating_sub(true as usize);
    let available = maximum.saturating_sub(prefix.len() + suffix.len());
    let disk: String = disk_name.chars().take(available).collect();
    let disk = disk.trim_end_matches('-');
    format!(
        "{prefix}{}{suffix}",
        if disk.is_empty() { "disk" } else { disk }
    )
}

fn disk_restore_postconditions(
    finding: &Finding,
    snapshot_name: &str,
) -> Vec<super::model::Condition> {
    let mut conditions = vec![
        planner::condition("exists", json!(true)),
        planner::condition("source_snapshot", json!(snapshot_name)),
    ];
    for field in [
        "type_url",
        "type",
        "size_gb",
        "labels",
        "description",
        "replica_zones",
        "resource_policies",
        "physical_block_size_bytes",
    ] {
        if let Some(value) = finding.evidence.get(field).filter(|value| !value.is_null()) {
            conditions.push(planner::condition(field, value.clone()));
        }
    }
    conditions
}

fn stable_preconditions(
    finding: &Finding,
    mut conditions: Vec<super::model::Condition>,
) -> Vec<super::model::Condition> {
    if let Some(resource_id) = finding.evidence.get("id").filter(|value| !value.is_null()) {
        conditions.push(planner::condition("resource_id", resource_id.clone()));
    }
    if let Some(created) = finding
        .evidence
        .get("creation_timestamp")
        .filter(|value| !value.is_null())
    {
        conditions.push(planner::condition("creation_timestamp", created.clone()));
    }
    if let Some(fingerprint) = finding
        .evidence
        .get("fingerprint")
        .filter(|value| !value.is_null())
    {
        conditions.push(planner::condition("fingerprint", fingerprint.clone()));
    }
    conditions
}

fn irreversible_action(
    id: String,
    finding: &Finding,
    kind: ActionKind,
    resource: ResourceLocator,
    parameters: Value,
    preconditions: Vec<super::model::Condition>,
) -> Action {
    Action {
        id,
        finding_id: Some(finding.id.clone()),
        kind,
        authorization: Authorization::Explicit,
        reversibility: Reversibility::Irreversible,
        resource,
        parameters,
        preconditions,
        postconditions: vec![planner::condition("exists", json!(false))],
        rollback: None,
        depends_on: Vec::new(),
    }
}

fn locator(finding: &Finding) -> ResourceLocator {
    let provider = crate::capabilities::provider(&finding.provider)
        .unwrap_or(crate::capabilities::ProviderId::Stado);
    let (name, location) = finding
        .resource
        .rsplit_once('@')
        .map_or((finding.resource.as_str(), None), |(name, location)| {
            (name, Some(location.to_string()))
        });
    ResourceLocator {
        provider,
        resource_type: finding.resource_type.to_string(),
        project: (provider == ProviderKind::Gcp && !config::project().is_empty())
            .then(|| config::project().to_string()),
        location,
        name: name.to_string(),
        reference: finding.resource.clone(),
    }
}
fn configured_backup_value() -> Value {
    let Some(path) = crate::config_file::config_path().ok().flatten() else {
        return Value::Null;
    };
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|root| root.pointer("/storage/backup").cloned())
        .unwrap_or(Value::Null)
}

fn configuration_findings(
    primary: &Endpoint,
    backup: Option<&Endpoint>,
    active: &[String],
    disabled: &[String],
) -> Vec<Finding> {
    let mut findings = Vec::new();
    let primary_local = primary.adapter() == Some(crate::capabilities::StorageAdapter::Local);
    let backup_config = configured_backup_value();
    if let Some(backup) = backup {
        if primary.describe() == backup.describe() {
            findings.push(finding(
                "storage-duplicate-backup",
                "high",
                "disable",
                "high",
                "stado",
                "storage-backup",
                backup.describe(),
                "primary and backup resolve to the same store, so the backup consumes work without adding a failure domain",
                json!({
                    "primary": primary.describe(),
                    "backup": backup.describe(),
                    "backup_config": backup_config.clone(),
                }),
            ));
        } else if primary_local
            && backup.adapter() == Some(crate::capabilities::StorageAdapter::Local)
        {
            findings.push(finding(
                "storage-local-only-backup",
                "medium",
                "move",
                "high",
                crate::capabilities::ProviderId::Local.as_str(),
                "storage-backup",
                backup.describe(),
                "a local primary and local backup remain in the same device failure domain; move the backup off-host or disable the misleading replica",
                json!({
                    "primary": primary.describe(),
                    "backup": backup.describe(),
                    "backup_config": backup_config,
                }),
            ));
        }
    }

    let active_remote: Vec<&str> = active
        .iter()
        .map(String::as_str)
        .filter(|provider| !crate::capabilities::ProviderId::Local.matches(provider))
        .collect();
    if primary_local && !active_remote.is_empty() {
        findings.push(finding(
            "local-storage-with-remote-compute",
            "high",
            "disable-or-migrate",
            "high",
            "stado",
            "configuration",
            primary.describe(),
            "remote agents cannot share a device-local queue reliably; migrate storage to a shared backend or keep remote providers disabled",
            json!({"active_remote_providers": active_remote}),
        ));
    }

    for provider in disabled {
        findings.push(finding(
            &format!("disabled-provider-{provider}"),
            "low",
            "review-deprovision",
            "medium",
            provider,
            "provider-fence",
            format!("provider:{provider}"),
            "the provider is fenced from scheduling; if that fence is permanent, remove its Stado-owned network, VM, reservation and credential resources",
            json!({"configured_state": "disabled"}),
        ));
    }
    findings
}

fn orphan_instance_findings(
    rows: &[instances::AuditInstanceRow],
    min_age_seconds: u64,
) -> Vec<Finding> {
    rows.iter()
        .filter(|row| row.is_orphan() && row.age_seconds >= min_age_seconds as f64)
        .map(|row| {
            let mut candidate = finding(
                &format!("orphan-vm-{}-{}", row.provider, row.reference),
                "high",
                "delete",
                "high",
                &row.provider,
                "agent-vm",
                row.reference.clone(),
                "the live agent VM has no running job and no unexpired provider lease in the authoritative store",
                json!({
                    "age_seconds": row.age_seconds,
                    "accelerator": row.accel,
                    "held_by": row.held_by,
                }),
            );
            candidate.automatic = true;
            candidate
        })
        .collect()
}

fn gcp_findings(
    report: &GcpInventoryReport,
    min_age_seconds: u64,
    now: DateTime<Utc>,
    provider_disabled: bool,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for disk in detail_items(report, "compute_disks", "disks") {
        if disk.get("unattached").and_then(Value::as_bool) != Some(true)
            || !old_enough(disk.get("creation_timestamp"), min_age_seconds, now)
        {
            continue;
        }
        let name = string_field(disk, "name");
        let location = first_nonempty(&[string_field(disk, "zone"), string_field(disk, "region")]);
        findings.push(finding(
            &format!("gcp-unattached-disk-{name}"),
            "medium",
            "review-delete",
            "medium",
            crate::capabilities::ProviderId::Gcp.as_str(),
            "persistent-disk",
            resource_at(&name, &location),
            "the persistent disk is unattached and older than the audit grace period; inspect its labels and snapshots before deletion",
            disk.clone(),
        ));
    }

    for address in detail_items(report, "static_addresses", "addresses") {
        let users_empty = address
            .get("users")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty);
        if address.get("status").and_then(Value::as_str) != Some("RESERVED")
            || !users_empty
            || !old_enough(address.get("creation_timestamp"), min_age_seconds, now)
        {
            continue;
        }
        let name = string_field(address, "name");
        let region = string_field(address, "region");
        findings.push(finding(
            &format!("gcp-unused-address-{name}"),
            "medium",
            "review-release",
            "medium",
            crate::capabilities::ProviderId::Gcp.as_str(),
            "static-address",
            resource_at(&name, &region),
            "the static address is reserved, has no users and is older than the audit grace period",
            address.clone(),
        ));
    }

    for group in detail_items(report, "managed_instance_groups", "managed_instance_groups") {
        let name = string_field(group, "name");
        let stado_owned = name.starts_with("wisent") || name.starts_with("stado");
        if !stado_owned
            || number_field(group, "target_size") != Some(u64::default())
            || !old_enough(group.get("creation_timestamp"), min_age_seconds, now)
        {
            continue;
        }
        let location =
            first_nonempty(&[string_field(group, "zone"), string_field(group, "region")]);
        findings.push(finding(
            &format!("gcp-empty-instance-group-{name}"),
            "medium",
            "review-delete",
            "medium",
            crate::capabilities::ProviderId::Gcp.as_str(),
            "managed-instance-group",
            resource_at(&name, &location),
            "the Stado/Wisent managed instance group has target size zero and is older than the audit grace period",
            group.clone(),
        ));
    }

    if provider_disabled {
        for reservation in detail_items(report, "compute_reservations", "reservations") {
            if !old_enough(reservation.get("creation_timestamp"), min_age_seconds, now) {
                continue;
            }
            let name = string_field(reservation, "name");
            let zone = string_field(reservation, "zone");
            findings.push(finding(
                &format!("gcp-disabled-provider-reservation-{name}"),
                "medium",
                "review-release",
                "medium",
                crate::capabilities::ProviderId::Gcp.as_str(),
                "compute-reservation",
                resource_at(&name, &zone),
                "GCP scheduling is disabled while this reservation remains; release it after confirming that no non-Stado workload consumes it",
                reservation.clone(),
            ));
        }
    }

    findings
}

fn detail_items<'a>(
    report: &'a GcpInventoryReport,
    probe_name: &str,
    key: &str,
) -> impl Iterator<Item = &'a Value> {
    report
        .probes
        .iter()
        .find(|probe| probe.name == probe_name && probe.state == "ok")
        .and_then(|probe: &ProbeReport| probe.detail.get(key))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

fn old_enough(value: Option<&Value>, min_age_seconds: u64, now: DateTime<Utc>) -> bool {
    let Some(timestamp) = value.and_then(Value::as_str) else {
        return false;
    };
    let Ok(created) = DateTime::parse_from_rfc3339(timestamp) else {
        return false;
    };
    now.signed_duration_since(created.with_timezone(&Utc))
        >= Duration::seconds(min_age_seconds.min(i64::MAX as u64) as i64)
}

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string()
}

fn number_field(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(|number| {
        number
            .as_u64()
            .or_else(|| number.as_str()?.parse::<u64>().ok())
    })
}

fn first_nonempty(values: &[String]) -> String {
    values
        .iter()
        .find(|value| !value.is_empty() && value.as_str() != "unknown")
        .cloned()
        .unwrap_or_default()
}

fn resource_at(name: &str, location: &str) -> String {
    if location.is_empty() {
        name.to_string()
    } else {
        format!("{name}@{location}")
    }
}

#[allow(clippy::too_many_arguments)]
fn finding(
    id: &str,
    severity: &'static str,
    action: &'static str,
    confidence: &'static str,
    provider: &str,
    resource_type: &'static str,
    resource: impl Into<String>,
    reason: &str,
    evidence: Value,
) -> Finding {
    Finding {
        id: id.to_string(),
        severity,
        action,
        confidence,
        provider: provider.to_string(),
        resource_type,
        resource: resource.into(),
        reason: reason.to_string(),
        evidence,
        automatic: false,
    }
}

fn summarize(findings: &[Finding], incomplete_sources: usize) -> Summary {
    let mut by_severity = BTreeMap::new();
    let mut by_action = BTreeMap::new();
    for finding in findings {
        *by_severity
            .entry(finding.severity.to_string())
            .or_insert(usize::default()) += 1;
        *by_action
            .entry(finding.action.to_string())
            .or_insert(usize::default()) += 1;
    }
    let state = match (findings.is_empty(), incomplete_sources == usize::default()) {
        (true, true) => "clean",
        (false, true) => "recommendations",
        (true, false) => "incomplete",
        (false, false) => "incomplete_with_recommendations",
    };
    Summary {
        state,
        findings: findings.len(),
        incomplete_sources,
        by_severity,
        by_action,
    }
}

fn severity_rank(severity: &str) -> u8 {
    match severity {
        "high" => u8::default(),
        "medium" => u8::from(true),
        "low" => u8::from(true).saturating_add(u8::from(true)),
        _ => u8::MAX,
    }
}

fn print_human(report: &RationalizationReport) {
    let rows: Vec<Vec<String>> = report
        .findings
        .iter()
        .map(|finding| {
            vec![
                finding.severity.to_uppercase(),
                finding.action.to_string(),
                finding.confidence.to_string(),
                finding.provider.clone(),
                finding.resource_type.to_string(),
                finding.resource.clone(),
                finding.reason.clone(),
            ]
        })
        .collect();
    table::print(
        &[
            "SEVERITY",
            "ACTION",
            "CONFIDENCE",
            "PROVIDER",
            "TYPE",
            "RESOURCE",
            "WHY",
        ],
        &rows,
    );

    let source_rows: Vec<Vec<String>> = report
        .sources
        .iter()
        .map(|source| {
            vec![
                source.name.clone(),
                source.state.to_string(),
                source
                    .detail
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            ]
        })
        .collect();
    table::print(&["SOURCE", "STATE", "ERROR"], &source_rows);
    println!(
        "\n{} recommendation(s); {} incomplete source(s); read-only, no changes applied.",
        report.summary.findings, report.summary.incomplete_sources
    );
}
