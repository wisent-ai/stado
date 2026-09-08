//! Which provider a finished job ran on, and which model it ran: the two
//! keys every cost bucket is grouped by.

use crate::models::Job;

/// local | gcp | azure | aws | unknown.
///
/// Explicit provider metadata wins. Legacy GCE records are recognized only
/// when their location has the unambiguous `region-zone` suffix; arbitrary
/// non-local references are never silently relabeled as GCP.
///
/// Python `_target_kind`.
pub fn target_kind(job: &Job) -> String {
    let reference = job.instance_ref.as_deref().unwrap_or("");
    if crate::capabilities::ProviderId::infer_from_instance_reference(reference)
        == Some(crate::capabilities::ProviderId::Local)
    {
        return crate::capabilities::ProviderId::Local.as_str().to_string();
    }
    let configured = crate::capabilities::variant(
        crate::capabilities::RuntimeFacet::Compute,
        job.provider.trim(),
    )
    .filter(|variant| {
        matches!(
            variant.adapter,
            crate::capabilities::RuntimeAdapter::Compute(adapter)
                if adapter.tracks_cloud_cost()
        )
    });
    if let Some(variant) = configured {
        return variant.id.to_string();
    }
    crate::capabilities::ProviderId::infer_from_instance_reference(reference)
        .map(|provider| provider.as_str().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Best-effort: extract --model 'X' out of the command line.
/// Python `_model_from_command`.
pub fn model_from_command(cmd: &str) -> String {
    let Some((_, rest)) = cmd.split_once("--model") else {
        return String::new();
    };
    let parts = rest.trim_start();
    let Some(first) = parts.chars().next() else {
        return String::new();
    };
    if first == '\'' || first == '"' {
        let after = &parts[first.len_utf8()..];
        if after.contains(first) {
            return after.split(first).next().unwrap_or("").to_string();
        }
        // Unterminated quote: Python falls through to the bare-token path.
        return after.split_whitespace().next().unwrap_or("").to_string();
    }
    parts.split_whitespace().next().unwrap_or("").to_string()
}
