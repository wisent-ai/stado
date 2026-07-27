//! `stado rationalize-resources` — read-only resource waste audit.
//!
//! Recommendations are deliberately conservative: a resource is proposed for
//! deletion or release only when Stado can identify it, prove it has no live
//! queue/lease owner where applicable, and establish a minimum age. Ambiguous
//! resources become review findings; this command never mutates a cloud or the
//! queue store.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use clap::Args;
use serde::Serialize;
use serde_json::{json, Value};

use super::{blast_radius, instances, table, CmdError};
use crate::config;
use crate::providers::gcp::inventory::{self as gcp_inventory, GcpInventoryReport, ProbeReport};
use crate::queue::copy::Endpoint;
use crate::queue::JobStorage;

#[derive(Args, Debug)]
pub struct RationalizeResourcesArgs {
    /// Ignore candidates younger than this age (`30m`, `24h`, `7d`).
    #[arg(long, default_value = "24h", value_parser = parse_age_seconds)]
    min_age: u64,
    /// Emit the complete versioned machine-readable report.
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
pub struct KillIrrationalResourcesArgs {
    /// Restrict cleanup to one configured VM provider.
    #[arg(long)]
    provider: Option<String>,
    /// Never delete a VM younger than this (`30m`, `24h`, `7d`).
    #[arg(
        long,
        alias = "older-than",
        default_value = "24h",
        value_parser = instances::parse_older_than
    )]
    min_age: Duration,
    /// Preview the deletion plan. This is already the default and overrides `--yes`.
    #[arg(long)]
    dry_run: bool,
    /// Apply the high-confidence deletion plan.
    #[arg(long)]
    yes: bool,
    /// Emit the machine-readable reaper report.
    #[arg(long)]
    json: bool,
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

pub async fn run(args: &RationalizeResourcesArgs) -> Result<(), CmdError> {
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

    let fleet_providers: Vec<String> = ["gcp", "azure", "aws"]
        .into_iter()
        .filter(|provider| configured.contains(*provider))
        .map(str::to_string)
        .collect();
    if fleet_providers.is_empty() {
        sources.push(SourceReport {
            name: "agent-vm-ownership".to_string(),
            state: "skipped",
            detail: json!({"reason": "no enumerable cloud or marketplace compute provider is configured"}),
        });
    } else if primary.kind == "local" {
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

    if configured.contains("gcp") {
        let options = blast_radius::gcp_inventory_options(&primary, backup.as_ref());
        let report = gcp_inventory::inspect(options).await;
        sources.push(SourceReport {
            name: "gcp-resource-inventory".to_string(),
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
            disabled.iter().any(|provider| provider == "gcp"),
        ));
    } else {
        sources.push(SourceReport {
            name: "gcp-resource-inventory".to_string(),
            state: "skipped",
            detail: json!({"reason": "GCP is absent from providers and providers_disabled"}),
        });
    }

    if configured.contains("aws") {
        sources.push(SourceReport {
            name: "aws-resource-inventory".to_string(),
            state: "unsupported",
            detail: json!({
                "reason": "EC2 VM ownership is covered, but EBS, Elastic IP and reservation inventory is not yet exposed",
                "remedy": "review AWS Cost Explorer and Resource Explorer before accepting this report as complete",
            }),
        });
    }
    if configured.contains("azure") {
        sources.push(SourceReport {
            name: "azure-resource-inventory".to_string(),
            state: "unsupported",
            detail: json!({
                "reason": "Azure VM ownership is covered, but managed disks, public IPs and reservation inventory is not yet exposed",
                "remedy": "review Azure Advisor cost recommendations before accepting this report as complete",
            }),
        });
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

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_human(&report);
    }
    Ok(())
}

pub async fn kill(args: &KillIrrationalResourcesArgs) -> Result<(), CmdError> {
    if args.min_age <= Duration::zero() {
        return Err(CmdError::usage(
            "--min-age must be greater than zero for a destructive cleanup",
        ));
    }
    let configured: BTreeSet<String> = config::wc_providers()
        .iter()
        .chain(config::wc_disabled_providers())
        .cloned()
        .collect();
    let supported = ["gcp", "azure", "aws"];
    let providers: Vec<String> = if let Some(selected) = args.provider.as_deref() {
        let selected = selected.trim();
        if !supported.contains(&selected) {
            return Err(CmdError::usage(format!(
                "{selected:?} has no provider VM deletion contract; use one of: {}",
                supported.join(", ")
            )));
        }
        if !configured.contains(selected) {
            return Err(CmdError::usage(format!(
                "provider {selected:?} is not present in providers or providers_disabled"
            )));
        }
        vec![selected.to_string()]
    } else {
        supported
            .into_iter()
            .filter(|provider| configured.contains(*provider))
            .map(str::to_string)
            .collect()
    };
    if providers.is_empty() {
        return Err(CmdError::click(
            "no configured GCP, Azure, or AWS VM fleet is available to clean",
        ));
    }

    let primary = Endpoint::configured_primary();
    if primary.kind == "local" {
        return Err(CmdError::click(
            "refusing orphan deletion: device-local storage is not an authoritative ownership \
             view for remote VMs; migrate the active queue to GCS, S3, or Azure Blob first",
        ));
    }

    let apply = args.yes && !args.dry_run;
    if !args.json {
        println!(
            "{} orphan agent VMs in {} older than {}s; review-only disks, addresses, groups, \
             reservations, storage, and provider fences will not be changed.",
            if apply { "Deleting" } else { "Previewing" },
            providers.join(", "),
            args.min_age.num_seconds(),
        );
    }
    instances::reap_irrational(&providers, args.min_age, apply, args.json).await?;
    if apply && !args.json {
        println!(
            "Applied only high-confidence orphan VM deletions. Run `stado rationalize-resources` \
             again to review non-automatic recommendations."
        );
    }
    Ok(())
}

fn configuration_findings(
    primary: &Endpoint,
    backup: Option<&Endpoint>,
    active: &[String],
    disabled: &[String],
) -> Vec<Finding> {
    let mut findings = Vec::new();
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
                json!({"primary": primary.describe(), "backup": backup.describe()}),
            ));
        } else if primary.kind == "local" && backup.kind == "local" {
            findings.push(finding(
                "storage-local-only-backup",
                "medium",
                "move",
                "high",
                "local",
                "storage-backup",
                backup.describe(),
                "a local primary and local backup remain in the same device failure domain; move the backup off-host or disable the misleading replica",
                json!({"primary": primary.describe(), "backup": backup.describe()}),
            ));
        }
    }

    let active_remote: Vec<&str> = active
        .iter()
        .map(String::as_str)
        .filter(|provider| *provider != "local")
        .collect();
    if primary.kind == "local" && !active_remote.is_empty() {
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
            "gcp",
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
            "gcp",
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
            "gcp",
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
                "gcp",
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

fn parse_age_seconds(raw: &str) -> Result<u64, String> {
    let raw = raw.trim();
    let (digits, multiplier) = match raw.as_bytes().last().copied() {
        Some(b's') => (&raw[..raw.len() - 1], 1_u64),
        Some(b'm') => (&raw[..raw.len() - 1], 60),
        Some(b'h') => (&raw[..raw.len() - 1], 60 * 60),
        Some(b'd') => (&raw[..raw.len() - 1], 24 * 60 * 60),
        _ => return Err("age must include a unit: s, m, h or d (for example 24h)".to_string()),
    };
    let count = digits
        .parse::<u64>()
        .map_err(|_| format!("invalid age {raw:?}"))?;
    count
        .checked_mul(multiplier)
        .ok_or_else(|| format!("age {raw:?} is too large"))
}
