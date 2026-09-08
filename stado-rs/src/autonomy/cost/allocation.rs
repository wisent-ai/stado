//! Attribution: live hourly prices onto inventory, inventory into buckets.
//!
//! [`enrich_inventory`] stamps each resource with the price the book quotes
//! for it, and [`build_allocation`] turns the priced resources into
//! [`CostEntry`] rows aggregated by provider, owner and workload.

use std::collections::BTreeMap;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::autonomy::model::{InventorySnapshot, ResourceRecord, SCHEMA_VERSION};
use crate::capabilities::ProviderId;
use crate::queue::{JobStorage, StorageError};

use super::prices::PriceBook;
use super::HOURS_PER_MONTH;

pub fn enrich_inventory(snapshot: &mut InventorySnapshot, prices: &PriceBook) {
    for resource in &mut snapshot.resources {
        enrich_resource(resource, prices);
    }
    for source in &mut snapshot.sources {
        for resource in &mut source.resources {
            enrich_resource(resource, prices);
        }
    }
}

fn enrich_resource(resource: &mut ResourceRecord, prices: &PriceBook) {
    let machine_type = resource
        .evidence
        .get("machine_type")
        .or_else(|| resource.evidence.get("instance_type"))
        .or_else(|| resource.evidence.pointer("/sku/name"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let accelerator = resource
        .evidence
        .get("accelerator_type")
        .and_then(Value::as_str)
        .unwrap_or("");
    let preemptible = resource
        .evidence
        .get("preemptible")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if let Some(quote) = prices.find_hourly(
        resource.provider,
        resource.region.as_deref(),
        machine_type,
        accelerator,
        preemptible,
    ) {
        resource.current_hourly_cost_usd = Some(quote.hourly_usd);
        resource.forecast_monthly_cost_usd = Some(quote.hourly_usd * HOURS_PER_MONTH);
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostEntry {
    pub schema_version: u16,
    pub entry_id: String,
    pub provider: ProviderId,
    pub service: String,
    pub resource_id: Option<String>,
    pub job_id: Option<String>,
    pub workload: Option<String>,
    pub owner: Option<String>,
    pub environment: Option<String>,
    pub region: Option<String>,
    pub usage_started_at: Option<String>,
    pub usage_ended_at: Option<String>,
    pub gross_cost_usd: f64,
    pub credits_usd: f64,
    pub net_cost_usd: f64,
    pub source: String,
    pub allocated: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CostBucket {
    pub gross_cost_usd: f64,
    pub credits_usd: f64,
    pub net_cost_usd: f64,
    pub entries: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AllocationReport {
    pub schema_version: u16,
    pub created_at: String,
    pub entries: Vec<CostEntry>,
    pub by_provider: BTreeMap<String, CostBucket>,
    pub by_owner: BTreeMap<String, CostBucket>,
    pub by_workload: BTreeMap<String, CostBucket>,
    pub allocated: CostBucket,
    pub unallocated: CostBucket,
}

pub async fn build_allocation(
    _store: &JobStorage,
    inventory: &InventorySnapshot,
) -> Result<AllocationReport, StorageError> {
    let mut entries = Vec::new();
    for resource in &inventory.resources {
        if resource.resource_type == "instance"
            && matches!(
                resource.state.to_ascii_lowercase().as_str(),
                "stopped" | "stopping" | "terminated" | "deallocated" | "deallocating"
            )
        {
            continue;
        }
        let Some(hourly) = resource.current_hourly_cost_usd else {
            continue;
        };
        entries.push(resource_cost_entry(resource, hourly));
    }
    Ok(aggregate_allocation(entries))
}

fn resource_cost_entry(resource: &ResourceRecord, hourly: f64) -> CostEntry {
    CostEntry {
        schema_version: SCHEMA_VERSION,
        entry_id: format!("resource:{}", resource.resource_id),
        provider: resource.provider,
        service: resource.resource_type.clone(),
        resource_id: Some(resource.resource_id.clone()),
        job_id: None,
        workload: resource.workload.clone(),
        owner: resource.owner.clone(),
        environment: resource.environment.clone(),
        region: resource.region.clone(),
        usage_started_at: None,
        usage_ended_at: None,
        gross_cost_usd: hourly,
        credits_usd: f64::default(),
        net_cost_usd: hourly,
        source: "live hourly price".to_string(),
        allocated: resource.owner.is_some() || resource.workload.is_some(),
    }
}

fn aggregate_allocation(entries: Vec<CostEntry>) -> AllocationReport {
    let mut report = AllocationReport {
        schema_version: SCHEMA_VERSION,
        created_at: Utc::now().to_rfc3339(),
        entries,
        by_provider: BTreeMap::new(),
        by_owner: BTreeMap::new(),
        by_workload: BTreeMap::new(),
        allocated: CostBucket::default(),
        unallocated: CostBucket::default(),
    };
    for entry in &report.entries {
        add_bucket(
            report
                .by_provider
                .entry(entry.provider.as_str().to_string())
                .or_default(),
            entry,
        );
        add_bucket(
            report
                .by_owner
                .entry(
                    entry
                        .owner
                        .clone()
                        .unwrap_or_else(|| "unallocated".to_string()),
                )
                .or_default(),
            entry,
        );
        add_bucket(
            report
                .by_workload
                .entry(
                    entry
                        .workload
                        .clone()
                        .unwrap_or_else(|| "unallocated".to_string()),
                )
                .or_default(),
            entry,
        );
        if entry.allocated {
            add_bucket(&mut report.allocated, entry);
        } else {
            add_bucket(&mut report.unallocated, entry);
        }
    }
    report
}

fn add_bucket(bucket: &mut CostBucket, entry: &CostEntry) {
    bucket.gross_cost_usd += entry.gross_cost_usd;
    bucket.credits_usd += entry.credits_usd;
    bucket.net_cost_usd += entry.net_cost_usd;
    bucket.entries += true as usize;
}
