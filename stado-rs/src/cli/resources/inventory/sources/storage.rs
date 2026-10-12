//! The queue's own storage endpoints, read through the shared bounded probe.

use serde_json::Value;

use crate::cli::blast_radius::{self, Reach};
use crate::cli::resources::inventory::model::SourceReport;
use crate::cli::resources::model::SourceCoverage;
use crate::queue::copy::Endpoint;

pub(in crate::cli::resources::inventory) async fn inspect_storage(
    primary: &Endpoint,
    backup: Option<&Endpoint>,
) -> SourceReport {
    let (primary, backup) = tokio::join!(
        blast_radius::storage_resource_report("primary", Some(primary)),
        blast_radius::storage_resource_report("backup", backup),
    );
    let degraded = [primary.0, backup.0]
        .into_iter()
        .any(|reach| matches!(reach, Reach::Unreachable | Reach::Degraded));
    SourceReport {
        name: "queue-storage",
        state: if degraded {
            SourceCoverage::Degraded
        } else {
            SourceCoverage::Ok
        },
        data: Value::Array(vec![primary.1, backup.1]),
        error: None,
    }
}
