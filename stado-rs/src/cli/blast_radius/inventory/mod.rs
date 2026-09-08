//! Provider inventory reads: what each independent probe can actually see.
//!
//! The GCP inventory options assembled here are shared with `resources` and
//! `autonomy`, so a project, region and bucket set is derived once from the
//! configured endpoints rather than restated per command.

mod credentials;
mod storage;

use std::collections::BTreeSet;

use crate::config;
use crate::providers::gcp::inventory::InventoryOptions;
use crate::queue::copy::Endpoint;

pub(super) use credentials::inspect_credential_store;
pub(super) use storage::inspect_storage_bounded;
pub(crate) use storage::storage_resource_report;

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
