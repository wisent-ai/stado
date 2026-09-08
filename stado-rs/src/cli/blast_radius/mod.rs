//! `stado blast-radius` — side-effect-free incident scope and DR readiness.
//!
//! The command keeps failure domains separate instead of collapsing them into
//! "the queue is empty": primary and backup stores, Skarbiec credentials,
//! live cloud resources and caller/runtime IAM, downstream consumers, and
//! backup namespace coverage. Provider probes are independent and paginated,
//! so one disabled API cannot hide the remaining project inventory.
//!
//! A backup is never selected automatically here. Queue state contains CAS
//! locks, leases and moving job records; transparent read redirection can make
//! two schedulers dispatch the same work from divergent stores. Promotion
//! must fence writers first, then select one backend for every participant.
//!
//! The report vocabulary lives here because every component of the command
//! names it: `command` assembles the whole report, `inventory` fills the
//! storage and credential-store parts, `analysis` derives coverage and
//! downstream impact from them, and `report` prints the human rendering.

mod analysis;
mod command;
mod inventory;
mod report;

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::providers::gcp::inventory::GcpInventoryReport;

pub use command::{run, BlastRadiusArgs};
pub(crate) use inventory::{gcp_inventory_options, storage_resource_report};

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
    "job-transitions/",
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
