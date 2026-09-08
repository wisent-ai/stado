//! The queue's own storage endpoints, read through the shared bounded probe.

use serde_json::Value;

use crate::cli::blast_radius;
use crate::cli::resources::inventory::model::SourceReport;
use crate::queue::copy::Endpoint;

pub(in crate::cli::resources::inventory) async fn inspect_storage(
    primary: &Endpoint,
    backup: Option<&Endpoint>,
) -> SourceReport {
    let (primary, backup) = tokio::join!(
        blast_radius::storage_resource_report("primary", Some(primary)),
        blast_radius::storage_resource_report("backup", backup),
    );
    let reports: Vec<Value> = vec![primary, backup];
    let state = if reports.iter().any(|report| {
        matches!(
            report.get("state").and_then(Value::as_str),
            Some("unreachable" | "degraded")
        )
    }) {
        "degraded"
    } else {
        "ok"
    };
    SourceReport {
        name: "queue-storage",
        state: state.to_string(),
        data: Value::Array(reports),
        error: None,
    }
}
