//! Per-provider VRAM tier ladders, machine-type shape inference and the
//! GCE accelerator defaults projected off those ladders.

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
