//! The storage probes: a bounded, per-prefix listing of one endpoint, plus the
//! provider-neutral projection `resources show` renders from the same read.

use std::collections::BTreeMap;

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::Value;

use crate::cli::blast_radius::{PrefixReport, StorageInspection, StorageReport};
use crate::queue::copy::{Endpoint, CANONICAL_PREFIXES};
use crate::queue::BlobBackend;

pub(in crate::cli::blast_radius) async fn inspect_storage_bounded(
    role: &str,
    endpoint: Option<&Endpoint>,
) -> StorageInspection {
    match tokio::time::timeout(
        crate::doctor::PROBE_TIMEOUT,
        inspect_storage(role, endpoint),
    )
    .await
    {
        Ok(inspection) => inspection,
        Err(_) => StorageInspection {
            report: StorageReport {
                role: role.to_string(),
                locator: endpoint.map(Endpoint::describe),
                state: "unreachable".to_string(),
                object_count: None,
                newest_object_at: None,
                error: Some(format!(
                    "storage inspection exceeded {:?}",
                    crate::doctor::PROBE_TIMEOUT
                )),
                prefixes: Vec::new(),
            },
            names: BTreeMap::new(),
        },
    }
}

/// Provider-neutral storage projection used by `resources show`.
pub(crate) async fn storage_resource_report(role: &str, endpoint: Option<&Endpoint>) -> Value {
    serde_json::to_value(inspect_storage_bounded(role, endpoint).await.report)
        .expect("storage report serialization is infallible")
}

async fn inspect_storage(role: &str, endpoint: Option<&Endpoint>) -> StorageInspection {
    let Some(endpoint) = endpoint else {
        return StorageInspection {
            report: StorageReport {
                role: role.to_string(),
                locator: None,
                state: "not_configured".to_string(),
                object_count: None,
                newest_object_at: None,
                error: None,
                prefixes: Vec::new(),
            },
            names: BTreeMap::new(),
        };
    };

    let locator = endpoint.describe();
    let backend = match endpoint.build().await {
        Ok(backend) => backend,
        Err(error) => {
            return StorageInspection {
                report: StorageReport {
                    role: role.to_string(),
                    locator: Some(locator),
                    state: "unreachable".to_string(),
                    object_count: None,
                    newest_object_at: None,
                    error: Some(error.to_string()),
                    prefixes: Vec::new(),
                },
                names: BTreeMap::new(),
            }
        }
    };

    inspect_backend(role, locator, &backend).await
}

async fn inspect_backend(
    role: &str,
    locator: String,
    backend: &std::sync::Arc<dyn BlobBackend>,
) -> StorageInspection {
    let mut reports = Vec::new();
    let mut names = BTreeMap::new();
    let mut newest = None;
    let mut total = usize::default();
    let mut first_error = None;

    for prefix in CANONICAL_PREFIXES {
        match backend.list_blobs_with_meta(prefix).await {
            Ok(blobs) => {
                let prefix_newest = blobs
                    .iter()
                    .filter_map(|blob| blob.updated.as_ref().cloned())
                    .max();
                newest = max_stamp(newest, prefix_newest);
                total = total.saturating_add(blobs.len());
                names.insert(
                    (*prefix).to_string(),
                    blobs.iter().map(|blob| blob.name.clone()).collect(),
                );
                reports.push(PrefixReport {
                    prefix: (*prefix).to_string(),
                    object_count: Some(blobs.len()),
                    newest_object_at: render_stamp(prefix_newest),
                    error: None,
                });
            }
            Err(error) => {
                let message = error.to_string();
                if first_error.is_none() {
                    first_error = Some(message.clone());
                }
                reports.push(PrefixReport {
                    prefix: (*prefix).to_string(),
                    object_count: None,
                    newest_object_at: None,
                    error: Some(message),
                });
                break;
            }
        }
    }

    let reachable = first_error.is_none();
    StorageInspection {
        report: StorageReport {
            role: role.to_string(),
            locator: Some(locator),
            state: if reachable {
                "reachable"
            } else {
                "unreachable"
            }
            .to_string(),
            object_count: reachable.then_some(total),
            newest_object_at: render_stamp(newest),
            error: first_error,
            prefixes: reports,
        },
        names,
    }
}

fn max_stamp(left: Option<DateTime<Utc>>, right: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(stamp), None) | (None, Some(stamp)) => Some(stamp),
        (None, None) => None,
    }
}

fn render_stamp(stamp: Option<DateTime<Utc>>) -> Option<String> {
    stamp.map(|value| value.to_rfc3339_opts(SecondsFormat::Secs, true))
}
