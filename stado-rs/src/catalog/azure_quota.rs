//! Azure quota-family catalog and the maps projected from it.

use std::collections::HashMap;
use std::sync::LazyLock;

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
