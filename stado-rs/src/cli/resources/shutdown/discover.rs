//! Ownership discovery for `--all-stado-owned`: one GCP inventory pass turned
//! into resource selectors, refused unless ownership can be proven complete.

use std::collections::BTreeSet;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::cli::blast_radius;
use crate::cli::resources::model::{InventorySnapshot, SourceSnapshot};
use crate::cli::resources::ShutdownArgs;
use crate::cli::CmdError;
use crate::providers::gcp::inventory as gcp_inventory;
use crate::queue::copy::Endpoint;

pub(super) async fn discover_owned(
    args: &ShutdownArgs,
) -> Result<(Vec<String>, InventorySnapshot), CmdError> {
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

fn tail(value: &str) -> &str {
    value.rsplit('/').next().unwrap_or(value)
}
