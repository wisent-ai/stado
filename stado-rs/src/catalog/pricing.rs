//! Per-GPU and GCE bundle rate tables plus the spot discount ladder.

use std::collections::HashMap;
use std::sync::LazyLock;

/// On-demand hourly rate per GPU, USD.
///
/// Pricing quirk: `nvidia-rtx-pro-6000` is an owned RTX PRO 6000 Blackwell
/// Workstation Edition (96 GB GDDR7, 600 W TGP). Hardware is sunk cost; the
/// hourly rate models only marginal electricity at California commercial
/// rates:
/// 0.6 kW x $0.30/kWh = $0.18/hr at full GPU power. Used by
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
