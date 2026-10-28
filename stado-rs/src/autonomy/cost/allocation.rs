//! Attribution: live hourly prices onto inventory, inventory into buckets.
//!
//! [`enrich_inventory`] stamps each resource with the price the book quotes
//! for it, and [`build_allocation`] turns the priced resources into
//! [`CostEntry`] rows aggregated by provider, owner and workload.

use std::collections::BTreeMap;

use chrono::{Datelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::autonomy::model::{InventorySnapshot, ResourceRecord};
use crate::capabilities::ProviderId;
use crate::queue::{JobStorage, StorageError};

use super::prices::PriceBook;
use super::HOURS_PER_DAY;

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

/// The bucket a resource with no owner or workload is reported under, so
/// what nobody claims is still totalled and visible.
const UNALLOCATED_BUCKET: &str = "unallocated";

fn enrich_resource(resource: &mut ResourceRecord, prices: &PriceBook) {
    // The price book is matched on the coordinates the provider's evidence
    // names; a resource whose evidence names no machine type, or no
    // accelerator, is priced with an empty coordinate, which the book
    // matches to nothing, and the resource stays unpriced. A preemptible
    // flag the evidence does not carry reads as on-demand pricing.
    let machine_type = match resource
        .evidence
        .get("machine_type")
        .or_else(|| resource.evidence.get("instance_type"))
        .or_else(|| resource.evidence.pointer("/sku/name"))
        .and_then(Value::as_str)
    {
        Some(machine_type) => machine_type,
        None => "",
    };
    let accelerator = match resource
        .evidence
        .get("accelerator_type")
        .and_then(Value::as_str)
    {
        Some(accelerator) => accelerator,
        None => "",
    };
    let preemptible = resource
        .evidence
        .get("preemptible")
        .and_then(Value::as_bool)
        == Some(true);
    if let Some(quote) = prices.find_hourly(
        resource.provider,
        resource.region.as_deref(),
        machine_type,
        accelerator,
        preemptible,
    ) {
        resource.current_hourly_cost_usd = Some(quote.hourly_usd);
        resource.forecast_monthly_cost_usd =
            Some(quote.hourly_usd * HOURS_PER_DAY * f64::from(Utc::now().num_days_in_month()));
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostEntry {
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
    pub entries: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AllocationReport {
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
        // A halted or released instance bills for its disk, which its own
        // disk record carries, not for compute.
        if resource.resource_type == crate::autonomy::model::ResourceKind::Instance
            && matches!(
                resource.phase,
                crate::providers::LifecyclePhase::Stopped | crate::providers::LifecyclePhase::Ended
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
        entry_id: format!("resource:{}", resource.resource_id),
        provider: resource.provider,
        service: resource.resource_type.to_string(),
        resource_id: Some(resource.resource_id.clone()),
        job_id: None,
        workload: resource.workload.clone(),
        owner: resource.owner.clone(),
        environment: resource.environment.clone(),
        region: resource.region.clone(),
        usage_started_at: None,
        usage_ended_at: None,
        // A live hourly price is the gross and the net at once: a provider's
        // credits are applied on its bill, not on a price quoted per hour.
        gross_cost_usd: hourly,
        credits_usd: f64::default(),
        net_cost_usd: hourly,
        source: "live hourly price".to_string(),
        allocated: resource.owner.is_some() || resource.workload.is_some(),
    }
}

fn aggregate_allocation(entries: Vec<CostEntry>) -> AllocationReport {
    let mut report = AllocationReport {
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
                .entry(match entry.owner.clone() {
                    Some(owner) => owner,
                    None => UNALLOCATED_BUCKET.to_string(),
                })
                .or_default(),
            entry,
        );
        add_bucket(
            report
                .by_workload
                .entry(match entry.workload.clone() {
                    Some(workload) => workload,
                    None => UNALLOCATED_BUCKET.to_string(),
                })
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
    bucket.entries.push(entry.entry_id.clone());
}
