//! The report vocabulary every component of the inventory shares.
//!
//! `SourceReport` is one fault-isolated source, and `ResourcesReport` is the
//! envelope `command` fills from `sources` and `human` prints.

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
pub(super) struct SourceReport {
    pub(super) name: &'static str,
    pub(super) state: String,
    pub(super) data: Value,
    pub(super) error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct ConfigurationReport {
    pub(super) active_compute: Vec<String>,
    pub(super) disabled_compute: Vec<String>,
    pub(super) primary_storage: String,
    pub(super) backup_storage: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct Summary {
    pub(super) state: &'static str,
    pub(super) configured_providers: usize,
    pub(super) visible_instances: usize,
    pub(super) confirmed_orphan_instances: usize,
    pub(super) storage_objects: usize,
    pub(super) incomplete_sources: usize,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ResourcesReport {
    pub(super) schema_version: u8,
    pub(super) generated_at: String,
    pub(super) read_only: bool,
    pub(super) configuration: ConfigurationReport,
    pub(super) summary: Summary,
    pub(super) storage: SourceReport,
    pub(super) compute: SourceReport,
    pub(super) host_registry: SourceReport,
    pub(super) gcp_inventory: SourceReport,
    pub(super) billing: SourceReport,
    pub(super) coverage_gaps: Vec<String>,
}
