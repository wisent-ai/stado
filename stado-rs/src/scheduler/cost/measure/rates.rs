//! Catalog $/hr lookup: the GPU SKU, the spot discount and the VM bundle
//! that together make the instance-hour a project is billed for.

use crate::catalog::{
    AZURE_VM_HOURLY_RATE_USD, GPU_HOURLY_RATE_USD, GPU_TYPE_TO_MACHINE_TYPE, SPOT_DISCOUNT,
    VM_BUNDLE_HOURLY_RATE_USD,
};

/// Total $/hr the project actually pays for one VM.
///
/// Azure: NC* SKUs bundle the GPU into a single line item, so we read
/// AZURE_VM_HOURLY_RATE_USD directly and skip the GPU+bundle sum.
///
/// GCP: bills the GPU SKU and the A2/N1/G2 Core+Ram SKUs separately, so
/// summing both yields the line-item total a user sees in Cloud Billing.
/// Falls back to GPU_TYPE_TO_MACHINE_TYPE to look up the bundle when
/// machine_type wasn't recorded on the Job.
///
/// Python `_hourly_rate_usd`.
pub fn hourly_rate_usd(gpu_type: &str, preemptible: bool, machine_type: &str) -> f64 {
    if machine_type.starts_with("Standard_") {
        let (on_demand, spot) = AZURE_VM_HOURLY_RATE_USD
            .get(machine_type)
            .copied()
            .unwrap_or((0.0, 0.0));
        return if preemptible { spot } else { on_demand };
    }
    let mut gpu = GPU_HOURLY_RATE_USD.get(gpu_type).copied().unwrap_or(0.0);
    if preemptible {
        gpu *= SPOT_DISCOUNT.get(gpu_type).copied().unwrap_or(0.5);
    }
    let mt = if machine_type.is_empty() {
        GPU_TYPE_TO_MACHINE_TYPE
            .get(gpu_type)
            .copied()
            .unwrap_or("")
    } else {
        machine_type
    };
    let bundle_pair = VM_BUNDLE_HOURLY_RATE_USD
        .get(mt)
        .copied()
        .unwrap_or((0.0, 0.0));
    let bundle = if preemptible {
        bundle_pair.1
    } else {
        bundle_pair.0
    };
    gpu + bundle
}
