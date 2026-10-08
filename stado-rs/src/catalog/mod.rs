//! GPU SKU catalog for Azure and GCE, and the per-provider sizing ladders.
//!
//! Port of `stado/_catalog/gpu_sku.py`. Covers every public NVIDIA / AMD GPU
//! VM family as of 2026-05 across both clouds: K80, P100, V100, T4, A10,
//! A100-40, A100-80, H100, H200, L4, B200, GB200, MI300X. Kept as pure data
//! tables behind `LazyLock` so they cost nothing until first use.
//!
//! [`GPU_SIZING`] is the one ladder table every consumer reads: the GCP, Azure
//! and AWS ladders declared in `sizing`, plus one ladder per GPU cloud vendor
//! projected from the offers its profile declares.

mod azure_machines;
mod azure_quota;
mod sizing;

use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;

pub use azure_machines::AZURE_VM_TO_ACCEL;
pub use azure_quota::{
    AzureQuotaFamily, AZURE_QUOTA_FAMILIES, AZURE_QUOTA_FAMILY_TO_ACCEL,
    AZURE_QUOTA_FAMILY_TO_MACHINE_TYPE,
};
pub use sizing::{MachineSpec, AWS_INSTANCE_TO_ACCEL, GPU_TYPE_TO_MACHINE_TYPE};

use sizing::{machine_type_provider as provider_by_shape, GPU_SIZING as CLOUD_SIZING};

/// Per-provider VRAM tier ladder: vram_gb -> (machine_type, accel_type),
/// tiers ascending so "smallest tier >= need" is a range scan. A vendor offer
/// sits at its accelerator's tier ([`accel_vram_tier`]); when two offers of
/// one vendor share a tier, the one its profile lists first is kept. A vendor
/// Stado cannot boot an agent on has no ladder: nothing is sized for it.
pub static GPU_SIZING: LazyLock<HashMap<&'static str, BTreeMap<i64, MachineSpec>>> =
    LazyLock::new(|| {
        let mut ladders = CLOUD_SIZING.clone();
        for vendor in crate::capabilities::GpuCloudVendor::ALL {
            let profile = crate::providers::gpu_cloud::profile(*vendor);
            if !profile.boots_agent() {
                continue;
            }
            let mut ladder = BTreeMap::new();
            for (machine, accel) in profile.offers {
                if let Some(tier) = accel_vram_tier(accel) {
                    ladder.entry(tier).or_insert((*machine, *accel));
                }
            }
            ladders.insert(vendor.provider().as_str(), ladder);
        }
        ladders
    });

/// The VRAM tier a vendor offer is filed under: the memory the accelerator
/// name declares (`nvidia-rtx-4090-24gb` sits at 24); for a name that
/// declares none, the largest tier the GCP, Azure or AWS ladder files it
/// under. `None` when neither says.
pub fn accel_vram_tier(accel: &str) -> Option<i64> {
    static DECLARED: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"-(?P<gb>\d+)gb$").expect("static accelerator-memory pattern compiles")
    });
    DECLARED
        .captures(accel)
        .and_then(|captures| captures.name("gb"))
        .and_then(|memory| memory.as_str().parse().ok())
        .or_else(|| {
            CLOUD_SIZING
                .values()
                .flat_map(|ladder| ladder.iter())
                .filter(|(_, (_, carried))| *carried == accel)
                .map(|(tier, _)| *tier)
                .max()
        })
}

/// Which provider a machine type belongs to: a GPU cloud vendor whose
/// profile offers exactly this type, else the GCP, Azure or AWS naming shape
/// (`Standard_*`, `family.size`, dash-separated lowercase). `None` when
/// nothing recognizes it, so an unknown pin is left alone.
pub fn machine_type_provider(machine_type: &str) -> Option<&'static str> {
    let value = machine_type.trim();
    crate::capabilities::GpuCloudVendor::ALL
        .iter()
        .find(|vendor| {
            crate::providers::gpu_cloud::profile(**vendor)
                .offers
                .iter()
                .any(|(offered, _)| *offered == value)
        })
        .map(|vendor| vendor.provider().as_str())
        .or_else(|| provider_by_shape(value))
}
