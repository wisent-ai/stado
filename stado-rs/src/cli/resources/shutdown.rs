//! Reversible shutdown planning. This module never mutates infrastructure.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::cli::{blast_radius, CmdError};
use crate::providers::gcp::inventory as gcp_inventory;
use crate::queue::copy::Endpoint;

use super::executors::Context;
use super::journal::Journal;
use super::model::{
    Action, ActionKind, Authorization, Intent, InventorySnapshot, OperationScope, ProviderKind,
    ResourceLocator, Reversibility, Rollback, SourceSnapshot,
};
use super::{planner, ShutdownArgs};

pub async fn run(args: &ShutdownArgs) -> Result<(), CmdError> {
    validate_project(&args.project)?;
    if !args.all_stado_owned && args.resource.is_empty() {
        return Err(CmdError::usage(
            "shutdown requires --all-stado-owned or at least one --resource",
        ));
    }
    let (selectors, inventory) = if args.all_stado_owned {
        discover_owned(args).await?
    } else {
        (
            args.resource.clone(),
            InventorySnapshot {
                snapshot_id: hex::encode(Sha256::digest(args.resource.join("\n").as_bytes())),
                complete: true,
                sources: vec![SourceSnapshot {
                    name: "operator-selectors".to_string(),
                    state: "ok".to_string(),
                    detail: json!({"resources": args.resource}),
                }],
            },
        )
    };
    let mut drafts = Vec::new();
    for selector in selectors {
        drafts.push(parse_selector(&args.project, &selector)?);
    }
    if drafts.is_empty() {
        return Err(CmdError::click(
            "shutdown discovery found no authoritative Stado-owned resources",
        ));
    }
    let context = Context::new(&drafts).await?;
    let mut actions = Vec::new();
    for draft in drafts {
        let observed = context.inspect(&draft).await?;
        if let Some(action) = finalize(draft, observed)? {
            actions.push(action);
        }
    }
    let plan = planner::new_plan(
        Intent::Shutdown,
        OperationScope {
            providers: [ProviderKind::Gcp].into_iter().collect(),
            projects: [args.project.clone()].into_iter().collect(),
            storage: Endpoint::configured_primary().describe(),
        },
        inventory,
        Vec::new(),
        actions,
    )?;
    let hash = planner::write_plan(&plan, &args.output)?;
    Journal::open().await?.create(&plan).await?;
    let summary = json!({
        "operation_id": plan.operation_id,
        "plan": args.output,
        "sha256": hash,
        "actions": plan.actions.len(),
        "reversible": plan.actions.iter().all(|action| action.rollback.is_some()),
        "safety": "no delete actions and no billing mutation",
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        println!(
            "shutdown plan {}: {} reversible action(s); SHA-256 {}",
            plan.operation_id,
            plan.actions.len(),
            hash
        );
        println!("written to {}", args.output.display());
    }
    Ok(())
}

async fn discover_owned(args: &ShutdownArgs) -> Result<(Vec<String>, InventorySnapshot), CmdError> {
    let primary = Endpoint::configured_primary();
    let backup = Endpoint::configured_backup();
    let mut options = blast_radius::gcp_inventory_options(&primary, backup.as_ref());
    options.project = args.project.clone();
    let report = gcp_inventory::inspect(options).await;
    if report.summary.critical_failures != usize::default() {
        return Err(CmdError::click(format!(
            "cannot prove complete Stado ownership: GCP inventory has {} critical failure(s)",
            report.summary.critical_failures
        )));
    }
    let mut selectors = BTreeSet::new();
    if let Some(instances) = probe_items(&report, "compute_instances", "instances") {
        for instance in instances {
            if instance.get("stado_managed").and_then(Value::as_bool) == Some(true) {
                if let (Some(name), Some(zone)) = (
                    instance.get("name").and_then(Value::as_str),
                    instance.get("zone").and_then(Value::as_str),
                ) {
                    selectors.insert(format!("gcp:instance:{}/{name}", tail(zone)));
                }
            }
        }
    }
    if let Some(groups) = probe_items(
        &report,
        "managed_instance_groups",
        "managed_instance_groups",
    ) {
        for group in groups {
            let Some(name) = group.get("name").and_then(Value::as_str) else {
                continue;
            };
            if !name.starts_with("stado-") && !name.starts_with("wisent-") {
                continue;
            }
            if let Some(zone) = group
                .get("zone")
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
            {
                selectors.insert(format!("gcp:zonal-mig:{}/{name}", tail(zone)));
            } else if let Some(region) = group
                .get("region")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
            {
                selectors.insert(format!("gcp:regional-mig:{}/{name}", tail(region)));
            }
        }
    }
    let scheduler = report
        .probes
        .iter()
        .find(|probe| probe.name == "cloud_scheduler")
        .ok_or_else(|| CmdError::click("GCP inventory omitted Cloud Scheduler ownership probe"))?;
    match scheduler.state.as_str() {
        "ok" => {
            if let Some(full_name) = scheduler.detail.get("name").and_then(Value::as_str) {
                if let Some(name) = full_name.rsplit('/').next() {
                    selectors.insert(format!("gcp:scheduler:{}/{name}", report.region));
                }
            }
        }
        "missing" => {}
        state => {
            return Err(CmdError::click(format!(
                "cannot prove complete Stado ownership: Cloud Scheduler probe is {state}"
            )))
        }
    }
    let detail = serde_json::to_value(&report)?;
    let snapshot_id = hex::encode(Sha256::digest(serde_json::to_vec(&detail)?));
    Ok((
        selectors.into_iter().collect(),
        InventorySnapshot {
            snapshot_id,
            complete: true,
            sources: vec![SourceSnapshot {
                name: "gcp-resource-inventory".to_string(),
                state: report.summary.state.clone(),
                detail,
            }],
        },
    ))
}

fn parse_selector(project: &str, selector: &str) -> Result<Action, CmdError> {
    let mut parts = selector.splitn("gcp".len(), ':');
    let provider = parts.next().unwrap_or_default();
    let resource_type = parts.next().unwrap_or_default();
    let locator = parts.next().unwrap_or_default();
    if provider != "gcp" || resource_type.is_empty() || locator.is_empty() {
        return Err(CmdError::usage(format!(
            "resource {selector:?} must use gcp:TYPE:LOCATION/NAME"
        )));
    }
    let (location, name) = if resource_type == "cloud-sql" {
        (None, locator)
    } else {
        let (location, name) = locator
            .split_once('/')
            .ok_or_else(|| CmdError::usage(format!("resource {selector:?} needs LOCATION/NAME")))?;
        (Some(location.to_string()), name)
    };
    validate_component(name, "resource name")?;
    if let Some(location) = location.as_deref() {
        validate_component(location, "resource location")?;
    }
    let (kind, scope, normalized_type) = match resource_type {
        "scheduler" => (ActionKind::PauseScheduler, "region", "scheduler-job"),
        "zonal-mig" => (
            ActionKind::ResizeManagedInstanceGroup,
            "zone",
            "managed-instance-group",
        ),
        "regional-mig" => (
            ActionKind::ResizeManagedInstanceGroup,
            "region",
            "managed-instance-group",
        ),
        "instance" => (ActionKind::StopInstance, "zone", "instance"),
        "cloud-sql" => (ActionKind::SuspendCloudSql, "global", "cloud-sql-instance"),
        other => {
            return Err(CmdError::usage(format!(
                "unsupported shutdown resource type {other:?}"
            )))
        }
    };
    Ok(Action {
        id: format!("action-{}", Uuid::new_v4().simple()),
        finding_id: None,
        kind,
        authorization: Authorization::Automatic,
        reversibility: Reversibility::Reversible,
        resource: ResourceLocator {
            provider: ProviderKind::Gcp,
            resource_type: normalized_type.to_string(),
            project: Some(project.to_string()),
            location,
            name: name.to_string(),
            reference: selector.to_string(),
        },
        parameters: json!({"scope": scope}),
        preconditions: Vec::new(),
        postconditions: Vec::new(),
        rollback: None,
        depends_on: Vec::new(),
    })
}

fn finalize(mut action: Action, observed: Value) -> Result<Option<Action>, CmdError> {
    if observed.get("exists").and_then(Value::as_bool) == Some(false) {
        return Err(CmdError::click(format!(
            "shutdown resource {} does not exist",
            action.resource.reference
        )));
    }
    match action.kind {
        ActionKind::PauseScheduler => {
            let state = required_string(&observed, "state", &action)?;
            if state == "PAUSED" {
                return Ok(None);
            }
            if state != "ENABLED" {
                return Err(CmdError::click(format!(
                    "Scheduler job {} is in unsupported state {state}",
                    action.resource.reference
                )));
            }
            action.preconditions = vec![planner::condition("state", json!(state))];
            action.postconditions = vec![planner::condition("state", json!("PAUSED"))];
            action.rollback = Some(Rollback {
                kind: ActionKind::ResumeScheduler,
                parameters: json!({}),
                preconditions: action.postconditions.clone(),
                postconditions: vec![planner::condition("state", json!(state))],
            });
        }
        ActionKind::ResizeManagedInstanceGroup => {
            let target = observed
                .get("target_size")
                .and_then(Value::as_i64)
                .ok_or_else(|| CmdError::click("managed group has no target size"))?;
            if target == i64::default() {
                return Ok(None);
            }
            action.parameters["target_size"] = json!(i64::default());
            action.preconditions = vec![planner::condition("target_size", json!(target))];
            action.postconditions = vec![planner::condition("target_size", json!(i64::default()))];
            action.rollback = Some(Rollback {
                kind: ActionKind::ResizeManagedInstanceGroup,
                parameters: json!({"target_size": target, "scope": action.parameters["scope"]}),
                preconditions: action.postconditions.clone(),
                postconditions: vec![planner::condition("target_size", json!(target))],
            });
        }
        ActionKind::StopInstance => {
            if observed.get("has_local_ssd").and_then(Value::as_bool) == Some(true) {
                return Err(CmdError::click(format!(
                    "refusing {}: Local SSD would require destructive discard",
                    action.resource.reference
                )));
            }
            let status = required_string(&observed, "status", &action)?;
            if status == "TERMINATED" {
                return Ok(None);
            }
            if status != "RUNNING" {
                return Err(CmdError::click(format!(
                    "instance {} is in unsupported state {status}",
                    action.resource.reference
                )));
            }
            action.preconditions = vec![
                planner::condition("status", json!(status)),
                planner::condition("has_local_ssd", json!(false)),
            ];
            action.postconditions = vec![planner::condition("status", json!("TERMINATED"))];
            action.rollback = Some(Rollback {
                kind: ActionKind::StartInstance,
                parameters: json!({}),
                preconditions: action.postconditions.clone(),
                postconditions: vec![planner::condition("status", json!("RUNNING"))],
            });
        }
        ActionKind::SuspendCloudSql => {
            let policy = required_string(&observed, "activation_policy", &action)?;
            if policy == "NEVER" {
                return Ok(None);
            }
            action.preconditions = vec![planner::condition("activation_policy", json!(policy))];
            action.postconditions = vec![planner::condition("activation_policy", json!("NEVER"))];
            action.rollback = Some(Rollback {
                kind: ActionKind::RestoreCloudSql,
                parameters: json!({"activation_policy": policy}),
                preconditions: action.postconditions.clone(),
                postconditions: vec![planner::condition("activation_policy", json!(policy))],
            });
        }
        _ => {
            return Err(CmdError::click(
                "shutdown planner received a destructive or unsupported action",
            ))
        }
    }
    for field in [
        "resource_id",
        "creation_timestamp",
        "fingerprint",
        "metadata_fingerprint",
        "etag",
        "settings_version",
    ] {
        if let Some(value) = observed.get(field).filter(|value| !value.is_null()) {
            action
                .preconditions
                .push(planner::condition(field, value.clone()));
        }
    }
    Ok(Some(action))
}

fn probe_items<'a>(
    report: &'a gcp_inventory::GcpInventoryReport,
    probe_name: &str,
    key: &str,
) -> Option<&'a Vec<Value>> {
    report
        .probes
        .iter()
        .find(|probe| probe.name == probe_name && probe.state == "ok")?
        .detail
        .get(key)?
        .as_array()
}

fn required_string<'a>(
    observed: &'a Value,
    key: &str,
    action: &Action,
) -> Result<&'a str, CmdError> {
    observed.get(key).and_then(Value::as_str).ok_or_else(|| {
        CmdError::click(format!(
            "resource {} has no {key}",
            action.resource.reference
        ))
    })
}

fn tail(value: &str) -> &str {
    value.rsplit('/').next().unwrap_or(value)
}

fn validate_project(project: &str) -> Result<(), CmdError> {
    validate_component(project, "GCP project")?;
    if project.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(CmdError::usage(
            "GCP project must be an explicit project id, not a numeric project number",
        ));
    }
    Ok(())
}

fn validate_component(value: &str, label: &str) -> Result<(), CmdError> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(CmdError::usage(format!("invalid {label} {value:?}")));
    }
    Ok(())
}
