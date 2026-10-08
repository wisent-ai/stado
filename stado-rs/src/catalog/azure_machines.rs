//! Azure VM size table: the accelerator each size carries. Prices are the
//! provider's live quotes (autonomy::cost::prices), not a table here.

use std::collections::HashMap;
use std::sync::LazyLock;

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
