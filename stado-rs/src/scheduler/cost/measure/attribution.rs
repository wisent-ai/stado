//! Which provider a finished job ran on: one of the two keys every cost
//! bucket is grouped by. The other, the model, is the one every part of
//! Stado reads from a command ([`crate::sizing::model_of`]).

use crate::models::Job;

/// The provider a job ran on: `local`, or a configured cloud provider id;
/// `None` when the job records neither a cost-tracked provider nor an
/// instance reference a provider can be read from.
///
/// Explicit provider metadata wins. Legacy GCE records are recognized only
/// when their location has the unambiguous `region-zone` suffix; arbitrary
/// non-local references are never silently relabeled as GCP.
///
/// Python `_target_kind`.
pub fn target_kind(job: &Job) -> Option<String> {
    // A job without an instance reference infers no provider from one.
    let reference = match job.instance_ref.as_deref() {
        Some(reference) => reference,
        None => "",
    };
    if crate::capabilities::ProviderId::infer_from_instance_reference(reference)
        == Some(crate::capabilities::ProviderId::Local)
    {
        return Some(crate::capabilities::ProviderId::Local.as_str().to_string());
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
        return Some(variant.id.to_string());
    }
    crate::capabilities::ProviderId::infer_from_instance_reference(reference)
        .map(|provider| provider.as_str().to_string())
}
