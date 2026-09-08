//! GPU SKU catalog for Azure and GCE.
//!
//! Port of `stado/_catalog/gpu_sku.py`. Covers every public NVIDIA / AMD GPU
//! VM family as of 2026-05 across both clouds: K80, P100, V100, T4, A10,
//! A100-40, A100-80, H100, H200, L4, B200, GB200, MI300X. Kept as pure data
//! tables behind `LazyLock` so they cost nothing until first use.

mod azure_machines;
mod azure_quota;
mod pricing;
mod sizing;

pub use azure_machines::{AZURE_VM_HOURLY_RATE_USD, AZURE_VM_TO_ACCEL};
pub use azure_quota::{
    AzureQuotaFamily, AZURE_QUOTA_FAMILIES, AZURE_QUOTA_FAMILY_TO_ACCEL,
    AZURE_QUOTA_FAMILY_TO_MACHINE_TYPE,
};
pub use pricing::{GPU_HOURLY_RATE_USD, SPOT_DISCOUNT, VM_BUNDLE_HOURLY_RATE_USD};
pub use sizing::{
    machine_type_provider, MachineSpec, AWS_INSTANCE_TO_ACCEL, GPU_SIZING, GPU_TYPE_TO_MACHINE_TYPE,
};
