//! GPU SKU catalog for Azure and GCE.
//!
//! Port of `stado/_catalog/gpu_sku.py`. Covers every public NVIDIA / AMD GPU
//! VM family as of 2026-05 across both clouds: K80, P100, V100, T4, A10,
//! A100-40, A100-80, H100, H200, L4, B200, GB200, MI300X. Kept as pure data
//! tables behind `LazyLock` so they cost nothing until first use.

use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;

/// (machine_type, accel_type) pair for one VRAM tier.
pub type MachineSpec = (&'static str, &'static str);

/// Per-provider VRAM tier ladder: vram_gb -> (machine_type, accel_type).
/// Tier keys are sorted ascending (BTreeMap) so "smallest tier >= need"
/// lookups are a simple range scan.
pub static GPU_SIZING: LazyLock<HashMap<&'static str, BTreeMap<i64, MachineSpec>>> =
    LazyLock::new(|| {
        HashMap::from([
            (
                crate::capabilities::ProviderId::Gcp.as_str(),
                BTreeMap::from([
                    (12, ("n1-standard-4", "nvidia-tesla-k80")),
                    (16, ("n1-standard-4", "nvidia-tesla-t4")),
                    (24, ("g2-standard-4", "nvidia-l4")),
                    (32, ("n1-standard-8", "nvidia-tesla-v100")),
                    (40, ("a2-highgpu-1g", "nvidia-tesla-a100")),
                    (80, ("a2-ultragpu-1g", "nvidia-a100-80gb")),
                    (94, ("a3-highgpu-1g", "nvidia-h100-80gb")),
                    (141, ("a3-ultragpu-8g", "nvidia-h200-141gb")),
                    (180, ("a4-highgpu-8g", "nvidia-b200-180gb")),
                    (192, ("a4x-highgpu-4g", "nvidia-gb200-192gb")),
                ]),
            ),
            (
                crate::capabilities::ProviderId::Azure.as_str(),
                BTreeMap::from([
                    (12, ("Standard_NC6", "nvidia-tesla-k80")),
                    (16, ("Standard_NC6s_v3", "nvidia-tesla-v100")),
                    (22, ("Standard_NC4as_T4_v3", "nvidia-tesla-t4")),
                    (24, ("Standard_NC8ads_A10_v4", "nvidia-a10")),
                    (40, ("Standard_NC24ads_A100_v4", "nvidia-a100-80gb")),
                    (80, ("Standard_NC24ads_A100_v4", "nvidia-a100-80gb")),
                    (94, ("Standard_NC40ads_H100_v5", "nvidia-h100-94gb")),
                    (141, ("Standard_ND96isr_H200_v5", "nvidia-h200-141gb")),
                    (180, ("Standard_ND96isr_B200_v6", "nvidia-b200-180gb")),
                    (192, ("Standard_ND96isr_MI300X_v5", "amd-mi300x-192gb")),
                ]),
            ),
            (
                crate::capabilities::ProviderId::Aws.as_str(),
                BTreeMap::from([
                    (16, ("g4dn.xlarge", "nvidia-tesla-t4")),
                    (24, ("g5.xlarge", "nvidia-a10")),
                    (48, ("g6e.xlarge", "nvidia-l40s")),
                    (80, ("p4de.24xlarge", "nvidia-a100-80gb")),
                    (94, ("p5.4xlarge", "nvidia-h100-80gb")),
                ]),
            ),
        ])
    });

/// Azure VM size -> (accel_type, gpu_count).
pub static AZURE_VM_TO_ACCEL: LazyLock<HashMap<&'static str, (&'static str, i64)>> =
    LazyLock::new(|| {
        HashMap::from([
            ("Standard_NC6", ("nvidia-tesla-k80", 1)),
            ("Standard_NC12", ("nvidia-tesla-k80", 2)),
            ("Standard_NC24", ("nvidia-tesla-k80", 4)),
            ("Standard_NC24r", ("nvidia-tesla-k80", 4)),
            ("Standard_NC6s_v2", ("nvidia-tesla-p100", 1)),
            ("Standard_NC12s_v2", ("nvidia-tesla-p100", 2)),
            ("Standard_NC24s_v2", ("nvidia-tesla-p100", 4)),
            ("Standard_NC24rs_v2", ("nvidia-tesla-p100", 4)),
            ("Standard_NC6s_v3", ("nvidia-tesla-v100", 1)),
            ("Standard_NC12s_v3", ("nvidia-tesla-v100", 2)),
            ("Standard_NC24s_v3", ("nvidia-tesla-v100", 4)),
            ("Standard_NC24rs_v3", ("nvidia-tesla-v100", 4)),
            ("Standard_NC4as_T4_v3", ("nvidia-tesla-t4", 1)),
            ("Standard_NC8as_T4_v3", ("nvidia-tesla-t4", 1)),
            ("Standard_NC16as_T4_v3", ("nvidia-tesla-t4", 1)),
            ("Standard_NC64as_T4_v3", ("nvidia-tesla-t4", 4)),
            ("Standard_NC8ads_A10_v4", ("nvidia-a10", 1)),
            ("Standard_NC16ads_A10_v4", ("nvidia-a10", 1)),
            ("Standard_NC32ads_A10_v4", ("nvidia-a10", 1)),
            ("Standard_NC24ads_A100_v4", ("nvidia-a100-80gb", 1)),
            ("Standard_NC48ads_A100_v4", ("nvidia-a100-80gb", 2)),
            ("Standard_NC96ads_A100_v4", ("nvidia-a100-80gb", 4)),
            ("Standard_NC40ads_H100_v5", ("nvidia-h100-94gb", 1)),
            ("Standard_NC80adis_H100_v5", ("nvidia-h100-94gb", 2)),
            ("Standard_NCC40ads_H100_v5", ("nvidia-h100-94gb", 1)),
            ("Standard_ND40rs_v2", ("nvidia-tesla-v100-32gb", 8)),
            ("Standard_ND96asr_v4", ("nvidia-tesla-a100", 8)),
            ("Standard_ND96amsr_A100_v4", ("nvidia-a100-80gb", 8)),
            ("Standard_ND96is_H100_v5", ("nvidia-h100-80gb", 8)),
            ("Standard_ND96isr_H100_v5", ("nvidia-h100-80gb", 8)),
            ("Standard_ND96isr_H200_v5", ("nvidia-h200-141gb", 8)),
            ("Standard_ND96isr_MI300X_v5", ("amd-mi300x-192gb", 8)),
            ("Standard_ND96isr_B200_v6", ("nvidia-b200-180gb", 8)),
            ("Standard_ND96isr_GB200_v6", ("nvidia-gb200-192gb", 8)),
            ("Standard_ND72isr_GB200_v6", ("nvidia-gb200-192gb", 8)),
            ("Standard_NV6ads_A10_v5", ("nvidia-a10", 1)),
            ("Standard_NV12ads_A10_v5", ("nvidia-a10", 1)),
            ("Standard_NV18ads_A10_v5", ("nvidia-a10", 1)),
            ("Standard_NV36ads_A10_v5", ("nvidia-a10", 1)),
            ("Standard_NV36adms_A10_v5", ("nvidia-a10", 1)),
            ("Standard_NV72ads_A10_v5", ("nvidia-a10", 2)),
            ("Standard_NV4ads_V710_v5", ("amd-radeonpro-v710", 1)),
            ("Standard_NV8ads_V710_v5", ("amd-radeonpro-v710", 1)),
            ("Standard_NV12ads_V710_v5", ("amd-radeonpro-v710", 1)),
            ("Standard_NV24ads_V710_v5", ("amd-radeonpro-v710", 1)),
            ("Standard_NV28adms_V710_v5", ("amd-radeonpro-v710", 1)),
        ])
    });

/// AWS instance type -> accel_type, projected from the canonical AWS sizing
/// ladder so a new instance cannot be schedulable but invisible to the AWS
/// provider adapter.
pub static AWS_INSTANCE_TO_ACCEL: LazyLock<HashMap<&'static str, &'static str>> =
    LazyLock::new(|| {
        GPU_SIZING
            .get(crate::capabilities::ProviderId::Aws.as_str())
            .into_iter()
            .flat_map(|tiers| tiers.values())
            .map(|(machine, accel)| (*machine, *accel))
            .collect()
    });

/// Which provider a machine type belongs to, by its naming shape.
///
/// Azure sizes are `Standard_*`, AWS instance types carry a family/size dot
/// (`g4dn.xlarge`), and GCE machine types are dash-separated lowercase
/// families (`e2-standard-8`). Returns `None` when nothing recognizes it, so
/// an unknown pin is left alone rather than silently rewritten.
pub fn machine_type_provider(machine_type: &str) -> Option<&'static str> {
    let value = machine_type.trim();
    if value.is_empty() {
        return None;
    }
    if value.starts_with("Standard_") {
        return Some(crate::capabilities::ProviderId::Azure.as_str());
    }
    if value.contains('.') {
        return Some(crate::capabilities::ProviderId::Aws.as_str());
    }
    if value.contains('-') && value == value.to_lowercase() {
        return Some(crate::capabilities::ProviderId::Gcp.as_str());
    }
    None
}

/// One Azure quota-family record. API spelling, accelerator semantics, and
/// the scheduler-compatible representative VM live together so the quota
/// reader and request writer cannot drift into separate family catalogs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AzureQuotaFamily {
    pub name: &'static str,
    pub accel: &'static str,
    pub machine_type: Option<&'static str>,
}

pub const AZURE_QUOTA_FAMILIES: &[AzureQuotaFamily] = &[
    AzureQuotaFamily {
        name: "standardNCFamily",
        accel: "nvidia-tesla-k80",
        machine_type: Some("Standard_NC6"),
    },
    AzureQuotaFamily {
        name: "standardNCSv2Family",
        accel: "nvidia-tesla-p100",
        machine_type: None,
    },
    AzureQuotaFamily {
        name: "standardNCSv3Family",
        accel: "nvidia-tesla-v100",
        machine_type: Some("Standard_NC6s_v3"),
    },
    AzureQuotaFamily {
        name: "standardNCPromoFamily",
        accel: "nvidia-tesla-k80",
        machine_type: None,
    },
    AzureQuotaFamily {
        name: "Standard NCASv3_T4 Family",
        accel: "nvidia-tesla-t4",
        machine_type: Some("Standard_NC4as_T4_v3"),
    },
    AzureQuotaFamily {
        name: "standardNCASv3Family",
        accel: "nvidia-tesla-t4",
        machine_type: Some("Standard_NC4as_T4_v3"),
    },
    AzureQuotaFamily {
        name: "standardNCASv4Family",
        accel: "nvidia-tesla-t4",
        machine_type: None,
    },
    AzureQuotaFamily {
        name: "StandardNCADSA10v4Family",
        accel: "nvidia-a10",
        machine_type: Some("Standard_NC8ads_A10_v4"),
    },
    AzureQuotaFamily {
        name: "StandardNCADSA100v4Family",
        accel: "nvidia-a100-80gb",
        machine_type: Some("Standard_NC24ads_A100_v4"),
    },
    AzureQuotaFamily {
        name: "StandardNCadsH100v5Family",
        accel: "nvidia-h100-94gb",
        machine_type: Some("Standard_NC40ads_H100_v5"),
    },
    AzureQuotaFamily {
        name: "StandardNCCads2023Family",
        accel: "nvidia-h100-94gb",
        machine_type: None,
    },
    AzureQuotaFamily {
        name: "standardNDSFamily",
        accel: "nvidia-tesla-p40",
        machine_type: None,
    },
    AzureQuotaFamily {
        name: "standardNDSv2Family",
        accel: "nvidia-tesla-v100-32gb",
        machine_type: None,
    },
    AzureQuotaFamily {
        name: "standardNDSv3Family",
        accel: "nvidia-tesla-v100",
        machine_type: None,
    },
    AzureQuotaFamily {
        name: "standard NDAMSv4_A100Family",
        accel: "nvidia-a100-80gb",
        machine_type: None,
    },
    AzureQuotaFamily {
        name: "Standard NDASv4_A100 Family",
        accel: "nvidia-tesla-a100",
        machine_type: None,
    },
    AzureQuotaFamily {
        name: "standardNDSH100v5Family",
        accel: "nvidia-h100-80gb",
        machine_type: None,
    },
    AzureQuotaFamily {
        name: "standardNDISRH200V5Family",
        accel: "nvidia-h200-141gb",
        machine_type: Some("Standard_ND96isr_H200_v5"),
    },
    AzureQuotaFamily {
        name: "standardNDISRGB200V6NDRFamily",
        accel: "nvidia-b200-180gb",
        machine_type: Some("Standard_ND96isr_B200_v6"),
    },
    AzureQuotaFamily {
        name: "standardNDISRGB300V6Family",
        accel: "nvidia-gb200-192gb",
        machine_type: None,
    },
    AzureQuotaFamily {
        name: "standardNDISRGB300G5V6Family",
        accel: "nvidia-gb200-192gb",
        machine_type: None,
    },
    AzureQuotaFamily {
        name: "standardNDISv5MI300XFamily",
        accel: "amd-mi300x-192gb",
        machine_type: Some("Standard_ND96isr_MI300X_v5"),
    },
    AzureQuotaFamily {
        name: "standardNVFamily",
        accel: "nvidia-tesla-m60",
        machine_type: None,
    },
    AzureQuotaFamily {
        name: "standardNVSv2Family",
        accel: "nvidia-tesla-m60",
        machine_type: None,
    },
    AzureQuotaFamily {
        name: "standardNVSv3Family",
        accel: "nvidia-tesla-m60",
        machine_type: None,
    },
    AzureQuotaFamily {
        name: "standardNVSv4Family",
        accel: "amd-radeonpro-v520",
        machine_type: None,
    },
    AzureQuotaFamily {
        name: "standardNVPromoFamily",
        accel: "nvidia-tesla-m60",
        machine_type: None,
    },
    AzureQuotaFamily {
        name: "StandardNVADSA10v5Family",
        accel: "nvidia-a10",
        machine_type: None,
    },
    AzureQuotaFamily {
        name: "StandardNVadsV710v5Family",
        accel: "amd-radeonpro-v710",
        machine_type: None,
    },
];

/// Azure quota family name -> accel_type. Derived from
/// [`AZURE_QUOTA_FAMILIES`].
pub static AZURE_QUOTA_FAMILY_TO_ACCEL: LazyLock<HashMap<&'static str, &'static str>> =
    LazyLock::new(|| {
        AZURE_QUOTA_FAMILIES
            .iter()
            .map(|family| (family.name, family.accel))
            .collect()
    });

/// Azure quota family -> scheduler-compatible representative VM size.
pub static AZURE_QUOTA_FAMILY_TO_MACHINE_TYPE: LazyLock<HashMap<&'static str, &'static str>> =
    LazyLock::new(|| {
        AZURE_QUOTA_FAMILIES
            .iter()
            .filter_map(|family| family.machine_type.map(|machine| (family.name, machine)))
            .collect()
    });

/// On-demand hourly rate per GPU, USD.
///
/// Pricing quirk: `nvidia-rtx-pro-6000` is an owned RTX PRO 6000 Blackwell
/// Workstation Edition (96 GB GDDR7, 600 W TGP). Hardware is sunk cost; the
/// hourly rate models only marginal electricity at California commercial
/// rates: 0.6 kW x $0.30/kWh = $0.18/hr at full GPU power. Used by
/// scheduler/cost.py for per-job cost accounting on this box.
pub static GPU_HOURLY_RATE_USD: LazyLock<HashMap<&'static str, f64>> = LazyLock::new(|| {
    HashMap::from([
        ("nvidia-tesla-k80", 0.45),
        ("nvidia-tesla-p100", 1.46),
        ("nvidia-tesla-p40", 1.30),
        ("nvidia-tesla-v100", 2.48),
        ("nvidia-tesla-v100-32gb", 3.06),
        ("nvidia-tesla-t4", 0.35),
        ("nvidia-l4", 0.71),
        ("nvidia-a10", 1.20),
        ("nvidia-tesla-a100", 2.93),
        ("nvidia-a100-80gb", 3.67),
        ("nvidia-h100-80gb", 11.06),
        ("nvidia-h100-94gb", 11.06),
        ("nvidia-h200-141gb", 13.50),
        ("nvidia-b200-180gb", 22.00),
        ("nvidia-gb200-192gb", 28.00),
        ("amd-mi300x-192gb", 9.00),
        ("amd-radeonpro-v520", 0.50),
        ("amd-radeonpro-v710", 0.70),
        ("nvidia-tesla-m60", 1.20),
        ("nvidia-rtx-pro-6000", 0.18),
    ])
});

/// Spot price as a fraction of on-demand (multiply, not subtract).
///
/// Pricing quirk: `nvidia-rtx-pro-6000` is owned hardware — no spot tier;
/// electricity costs the same regardless, hence 1.0.
pub static SPOT_DISCOUNT: LazyLock<HashMap<&'static str, f64>> = LazyLock::new(|| {
    HashMap::from([
        ("nvidia-tesla-k80", 0.30),
        ("nvidia-tesla-p100", 0.30),
        ("nvidia-tesla-p40", 0.30),
        ("nvidia-tesla-v100", 0.30),
        ("nvidia-tesla-v100-32gb", 0.30),
        ("nvidia-tesla-t4", 0.49),
        ("nvidia-l4", 0.40),
        ("nvidia-a10", 0.40),
        ("nvidia-tesla-a100", 0.49),
        ("nvidia-a100-80gb", 0.54),
        ("nvidia-h100-80gb", 0.45),
        ("nvidia-h100-94gb", 0.45),
        ("nvidia-h200-141gb", 0.50),
        ("nvidia-b200-180gb", 0.55),
        ("nvidia-gb200-192gb", 0.60),
        ("amd-mi300x-192gb", 0.50),
        ("amd-radeonpro-v520", 0.30),
        ("amd-radeonpro-v710", 0.30),
        ("nvidia-tesla-m60", 0.30),
        ("nvidia-rtx-pro-6000", 1.0),
    ])
});

/// GCE machine-type bundle rates: (on_demand, spot) USD/hour. Note the
/// bundle rate is the full VM (GPU + CPU + memory), unlike
/// [`GPU_HOURLY_RATE_USD`] which is per-GPU.
pub static VM_BUNDLE_HOURLY_RATE_USD: LazyLock<HashMap<&'static str, (f64, f64)>> =
    LazyLock::new(|| {
        HashMap::from([
            ("a2-highgpu-1g", (1.50, 0.37)),
            ("a2-ultragpu-1g", (1.85, 0.55)),
            ("a2-highgpu-2g", (3.00, 0.74)),
            ("a2-ultragpu-2g", (3.70, 1.10)),
            ("a2-highgpu-4g", (6.00, 1.48)),
            ("a2-ultragpu-4g", (7.40, 2.20)),
            ("a2-highgpu-8g", (12.00, 2.96)),
            ("a2-ultragpu-8g", (14.80, 4.40)),
            ("a3-highgpu-1g", (3.00, 1.20)),
            ("a3-highgpu-2g", (6.00, 2.40)),
            ("a3-highgpu-4g", (12.00, 4.80)),
            ("a3-highgpu-8g", (8.00, 3.20)),
            ("a3-megagpu-8g", (10.00, 4.00)),
            ("a3-edgegpu-8g", (9.00, 3.60)),
            ("a3-ultragpu-8g", (12.00, 4.80)),
            ("a4-highgpu-8g", (20.00, 8.00)),
            ("a4x-highgpu-4g", (24.00, 9.60)),
            ("n1-standard-4", (0.20, 0.06)),
            ("n1-standard-8", (0.40, 0.12)),
            ("g2-standard-4", (0.30, 0.12)),
            ("g2-standard-8", (0.60, 0.24)),
        ])
    });

/// GCE accel_type -> default machine_type carrying that GPU. Scheduler tier
/// defaults are projected from [`GPU_SIZING`]; only accelerator families not
/// present in that ladder are declared as supplemental defaults here.
pub static GPU_TYPE_TO_MACHINE_TYPE: LazyLock<HashMap<&'static str, &'static str>> =
    LazyLock::new(|| {
        let mut defaults = GPU_SIZING
            .get(crate::capabilities::ProviderId::Gcp.as_str())
            .into_iter()
            .flat_map(|tiers| tiers.values())
            .map(|(machine, accel)| (*accel, *machine))
            .collect::<HashMap<_, _>>();
        defaults.extend([
            ("nvidia-tesla-p100", "n1-standard-8"),
            ("nvidia-tesla-p40", "n1-standard-8"),
            ("nvidia-tesla-v100-32gb", "n1-standard-8"),
            ("nvidia-a10", "n1-standard-8"),
            ("amd-mi300x-192gb", "a4-highgpu-8g"),
        ]);
        defaults
    });

/// Azure VM size -> (on_demand, spot) USD/hour bundle rates.
pub static AZURE_VM_HOURLY_RATE_USD: LazyLock<HashMap<&'static str, (f64, f64)>> =
    LazyLock::new(|| {
        HashMap::from([
            ("Standard_NC4as_T4_v3", (0.526, 0.105)),
            ("Standard_NC8as_T4_v3", (0.752, 0.150)),
            ("Standard_NC16as_T4_v3", (1.204, 0.241)),
            ("Standard_NC64as_T4_v3", (4.352, 0.870)),
            ("Standard_NC8ads_A10_v4", (0.91, 0.18)),
            ("Standard_NC16ads_A10_v4", (1.81, 0.36)),
            ("Standard_NC32ads_A10_v4", (3.62, 0.72)),
            ("Standard_NC24ads_A100_v4", (3.673, 0.735)),
            ("Standard_NC48ads_A100_v4", (7.346, 1.470)),
            ("Standard_NC96ads_A100_v4", (14.692, 2.940)),
            ("Standard_NC40ads_H100_v5", (6.98, 1.40)),
            ("Standard_NC80adis_H100_v5", (13.96, 2.80)),
            ("Standard_NCC40ads_H100_v5", (7.50, 1.50)),
            ("Standard_ND40rs_v2", (22.0, 6.6)),
            ("Standard_ND96asr_v4", (27.20, 8.16)),
            ("Standard_ND96amsr_A100_v4", (32.77, 9.83)),
            ("Standard_ND96is_H100_v5", (84.00, 25.20)),
            ("Standard_ND96isr_H100_v5", (98.32, 29.50)),
            ("Standard_ND96isr_H200_v5", (110.00, 33.00)),
            ("Standard_ND96isr_MI300X_v5", (60.00, 18.00)),
            ("Standard_ND96isr_B200_v6", (176.00, 52.80)),
            ("Standard_ND96isr_GB200_v6", (220.00, 66.00)),
            ("Standard_ND72isr_GB200_v6", (165.00, 49.50)),
            ("Standard_NV6ads_A10_v5", (0.45, 0.09)),
            ("Standard_NV12ads_A10_v5", (0.90, 0.18)),
            ("Standard_NV18ads_A10_v5", (1.35, 0.27)),
            ("Standard_NV36ads_A10_v5", (2.70, 0.54)),
            ("Standard_NV36adms_A10_v5", (4.10, 0.82)),
            ("Standard_NV72ads_A10_v5", (5.40, 1.08)),
            ("Standard_NV4ads_V710_v5", (0.34, 0.07)),
            ("Standard_NV8ads_V710_v5", (0.68, 0.14)),
            ("Standard_NV12ads_V710_v5", (1.02, 0.21)),
            ("Standard_NV24ads_V710_v5", (2.04, 0.41)),
            ("Standard_NV28adms_V710_v5", (3.06, 0.61)),
        ])
    });

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn gpu_sizing_shape() {
        assert_eq!(GPU_SIZING.len(), 3);
        assert_eq!(GPU_SIZING["gcp"].len(), 10);
        assert_eq!(GPU_SIZING["azure"].len(), 10);
        assert_eq!(GPU_SIZING["aws"].len(), 5);
        assert_eq!(
            GPU_SIZING["gcp"][&80],
            ("a2-ultragpu-1g", "nvidia-a100-80gb")
        );
        assert_eq!(
            GPU_SIZING["azure"][&192],
            ("Standard_ND96isr_MI300X_v5", "amd-mi300x-192gb")
        );
        // Tier keys are sorted ascending per provider.
        for sizing in GPU_SIZING.values() {
            let keys: Vec<i64> = sizing.keys().copied().collect();
            let mut sorted = keys.clone();
            sorted.sort_unstable();
            assert_eq!(keys, sorted);
        }
    }

    #[test]
    fn azure_vm_tables() {
        assert_eq!(AZURE_VM_TO_ACCEL.len(), 46);
        assert_eq!(
            AZURE_VM_TO_ACCEL["Standard_NC64as_T4_v3"],
            ("nvidia-tesla-t4", 4)
        );
        assert_eq!(
            AZURE_VM_TO_ACCEL["Standard_ND96isr_B200_v6"],
            ("nvidia-b200-180gb", 8)
        );
        assert_eq!(AZURE_QUOTA_FAMILY_TO_ACCEL.len(), 29);
        assert_eq!(
            AZURE_QUOTA_FAMILY_TO_ACCEL["standardNDISv5MI300XFamily"],
            "amd-mi300x-192gb"
        );
    }

    #[test]
    fn aws_instance_to_accel() {
        assert_eq!(AWS_INSTANCE_TO_ACCEL.len(), 5);
        assert_eq!(AWS_INSTANCE_TO_ACCEL["g4dn.xlarge"], "nvidia-tesla-t4");
        assert_eq!(AWS_INSTANCE_TO_ACCEL["p5.4xlarge"], "nvidia-h100-80gb");
    }

    #[test]
    fn rates() {
        assert_eq!(GPU_HOURLY_RATE_USD.len(), 20);
        assert_eq!(GPU_HOURLY_RATE_USD["nvidia-h100-80gb"], 11.06);
        assert_eq!(GPU_HOURLY_RATE_USD["nvidia-rtx-pro-6000"], 0.18);
        assert_eq!(SPOT_DISCOUNT.len(), 20);
        assert_eq!(SPOT_DISCOUNT["nvidia-rtx-pro-6000"], 1.0);
        assert_eq!(VM_BUNDLE_HOURLY_RATE_USD.len(), 21);
        assert_eq!(VM_BUNDLE_HOURLY_RATE_USD["a3-highgpu-8g"], (8.00, 3.20));
        assert_eq!(GPU_TYPE_TO_MACHINE_TYPE["nvidia-l4"], "g2-standard-4");
        for &(machine, accel) in GPU_SIZING[crate::capabilities::ProviderId::Gcp.as_str()].values()
        {
            assert_eq!(GPU_TYPE_TO_MACHINE_TYPE[accel], machine);
        }
        assert_eq!(AZURE_VM_HOURLY_RATE_USD.len(), 34);
        assert_eq!(
            AZURE_VM_HOURLY_RATE_USD["Standard_ND96isr_GB200_v6"],
            (220.00, 66.00)
        );
    }
}
