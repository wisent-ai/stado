//! The reading, as the `--json` document.

use serde_json::{json, Map, Value};

use crate::deploy::host_channel;
use crate::targets::ComputeTarget;

use super::RelocateReading;

/// The reading as the `--json` report, in the shared host report shape.
pub fn to_report(
    target: &ComputeTarget,
    reading: &RelocateReading,
    namespace: &str,
    applied: bool,
) -> Map<String, Value> {
    let mut report = host_channel::base_report(target);
    report.insert("namespace".to_string(), json!(namespace));
    report.insert("applied".to_string(), json!(applied));
    report.insert(
        "store".to_string(),
        json!({
            "root": reading.store_root,
            "source_prefix": reading.source_prefix,
            "destination_prefix": reading.destination_prefix,
            "missing_root": reading.missing_root,
            "no_hasher": reading.no_hasher,
        }),
    );
    report.insert(
        "totals".to_string(),
        json!({
            "scanned": reading.scanned,
            "decided": reading.decided,
            "moved": reading.moved,
            "moved_bytes": reading.moved_bytes,
            "refused": reading.refused,
            "pruned_directories": reading.pruned_directories,
            "stale_uris": reading.stale_uris,
            "repaired_uris": reading.repaired_uris,
            // A pass whose closing marker never arrived states so, because the
            // totals of a truncated read are a lower bound and reading them as
            // the answer is how a half-finished relocation looks finished.
            "complete": reading.complete,
            "remaining": (reading.scanned - reading.decided).max(0),
        }),
    );
    report.insert(
        "objects".to_string(),
        Value::Array(
            reading
                .objects
                .iter()
                .map(|item| {
                    json!({
                        "outcome": item.outcome,
                        "bytes": item.bytes,
                        "sha256": item.sha256,
                        "source_key": item.source_key,
                        "destination_key": item.destination_key,
                    })
                })
                .collect(),
        ),
    );
    report.insert(
        "metadata".to_string(),
        Value::Array(
            reading
                .metadata
                .iter()
                .map(|item| {
                    json!({
                        "outcome": item.outcome,
                        "source_key": item.source_key,
                    })
                })
                .collect(),
        ),
    );
    report
}
