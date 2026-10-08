//! GCP project, bucket, region and image settings.

use std::sync::LazyLock;

use crate::config::{
    resolve_capability_binding, resolve_capability_list_binding, resolve_storage_binding,
    DEFAULT_GCP_PROJECT, DEFAULT_GCP_REGION, DEFAULT_GCP_REGIONS, DEFAULT_GCS_BUCKET,
};

static PROJECT: LazyLock<String> = LazyLock::new(|| {
    resolve_capability_binding(
        crate::capabilities::RuntimeFacet::Compute,
        crate::capabilities::ProviderId::Gcp.as_str(),
        "project",
        false,
        DEFAULT_GCP_PROJECT,
    )
});
static BUCKET: LazyLock<String> = LazyLock::new(|| {
    resolve_storage_binding(
        crate::capabilities::StorageAdapter::Gcs,
        "bucket",
        false,
        DEFAULT_GCS_BUCKET,
    )
});
static REGION: LazyLock<String> = LazyLock::new(|| {
    resolve_capability_binding(
        crate::capabilities::RuntimeFacet::Compute,
        crate::capabilities::ProviderId::Gcp.as_str(),
        "region",
        false,
        DEFAULT_GCP_REGION,
    )
});

/// GCP project id (env `GCP_PROJECT`).
pub fn project() -> &'static str {
    PROJECT.as_str()
}

/// Queue storage bucket (env `WC_BUCKET`).
pub fn bucket() -> &'static str {
    BUCKET.as_str()
}

/// Primary GCP region (env `GCP_REGION`).
pub fn region() -> &'static str {
    REGION.as_str()
}

static REGIONS: LazyLock<Vec<String>> = LazyLock::new(|| {
    resolve_capability_list_binding(
        crate::capabilities::RuntimeFacet::Compute,
        crate::capabilities::ProviderId::Gcp.as_str(),
        "regions",
        DEFAULT_GCP_REGIONS,
    )
});

/// Multi-region dispatch (env `GCP_REGIONS`, comma-separated). Every region
/// listed here is queried for live quota AND iterated by the GCP provider
/// when creating instances. Each region carries a default GCP-issued quota
/// (16 preemptible A100, 4 preemptible A100-80GB, 8 preemptible L4, 8
/// preemptible T4) so spreading across these 5 regions lifts total
/// parallel-VM ceiling from ~28 to ~140 without any quota-increase request.
/// Override with GCP_REGIONS=us-central1,europe-west4 (comma-separated) to
/// narrow the dispatch surface for testing.
pub fn regions() -> &'static [String] {
    &REGIONS
}

static IMAGE: LazyLock<String> = LazyLock::new(|| {
    resolve_capability_binding(
        crate::capabilities::RuntimeFacet::Compute,
        crate::capabilities::ProviderId::Gcp.as_str(),
        "image",
        false,
        "",
    )
});
static IMAGE_PROJECT: LazyLock<String> = LazyLock::new(|| {
    resolve_capability_binding(
        crate::capabilities::RuntimeFacet::Compute,
        crate::capabilities::ProviderId::Gcp.as_str(),
        "image-project",
        false,
        "",
    )
});

/// The boot image a rented GCE machine starts from (env `GCP_IMAGE`), as the
/// deployment declares it; empty when undeclared, and the GCP provider then
/// refuses to create a machine by name.
pub fn gcp_image() -> &'static str {
    IMAGE.as_str()
}

/// The project that publishes [`gcp_image`] (env `GCP_IMAGE_PROJECT`).
pub fn gcp_image_project() -> &'static str {
    IMAGE_PROJECT.as_str()
}
