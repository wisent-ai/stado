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

pub const DEFAULT_IMAGE: &str = "pytorch-2-9-cu129-ubuntu-2204-nvidia-580-v20260408";
pub const DEFAULT_IMAGE_PROJECT: &str = "deeplearning-platform-release";
pub const DEFAULT_CPU_IMAGE_FAMILY: &str = "ubuntu-2204-lts";
pub const DEFAULT_CPU_IMAGE_PROJECT: &str = "ubuntu-os-cloud";
pub const DEFAULT_BOOT_DISK_GB: i64 = 200;
