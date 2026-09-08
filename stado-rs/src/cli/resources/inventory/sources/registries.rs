//! The two registries of declared assets: Stado's own host registry, and the
//! fault-isolated GCP resource probes.

use serde_json::{json, Value};

use crate::cli::blast_radius;
use crate::cli::resources::inventory::model::SourceReport;
use crate::providers::gcp::inventory as gcp_inventory;
use crate::queue::copy::Endpoint;

pub(in crate::cli::resources::inventory) async fn inspect_registry() -> SourceReport {
    match crate::targets::load_registry_auto().await {
        Ok(registry) => SourceReport {
            name: "host-registry",
            state: "ok".to_string(),
            data: json!({
                "target_count": registry.targets.len(),
                "coordinator_count": registry.coordinators.len(),
                "targets": registry.targets,
                "coordinators": registry.coordinators,
            }),
            error: None,
        },
        Err(error) => SourceReport {
            name: "host-registry",
            state: "blocked".to_string(),
            data: Value::Null,
            error: Some(error.to_string()),
        },
    }
}

pub(in crate::cli::resources::inventory) async fn inspect_gcp(
    enabled: bool,
    primary: &Endpoint,
    backup: Option<&Endpoint>,
) -> SourceReport {
    if !enabled {
        return SourceReport {
            name: "gcp-inventory",
            state: "skipped".to_string(),
            data: Value::Null,
            error: None,
        };
    }
    let options = blast_radius::gcp_inventory_options(primary, backup);
    let report = gcp_inventory::inspect(options).await;
    let state = report.summary.state.clone();
    SourceReport {
        name: "gcp-inventory",
        state,
        data: serde_json::to_value(report).expect("GCP inventory serialization is infallible"),
        error: None,
    }
}
