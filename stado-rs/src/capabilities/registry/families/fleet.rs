//! Inventory, quota, billing and artifact variants.

use crate::capabilities::catalog::ProviderId;
use crate::capabilities::config::variants::{
    AWS_COMPUTE_CONFIG, AZURE_COMPUTE_CONFIG, GCP_COMPUTE_CONFIG,
};
use crate::capabilities::registry::CapabilityVariant;
use crate::capabilities::runtime::{
    BillingAdapter, GpuCloudVendor, InventoryAdapter, QuotaAdapter, RuntimeAdapter,
};

/// A GPU cloud vendor's agent machines as an inventory source.
const fn gpu_cloud_inventory(vendor: GpuCloudVendor) -> CapabilityVariant {
    CapabilityVariant {
        id: vendor.provider().as_str(),
        aliases: vendor.provider().aliases(),
        provider: Some(vendor.provider()),
        implementation: "cli::resources::inventory",
        summary: "Enumerate Stado-owned agent machines on this GPU cloud vendor; volumes, \
                  addresses and machines Stado did not launch are not enumerated.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::Inventory(InventoryAdapter::GpuCloud(vendor)),
        config: crate::providers::gpu_cloud::profile(vendor).config,
    }
}

pub(in crate::capabilities::registry) const INVENTORY: &[CapabilityVariant] = &[
    CapabilityVariant {
        id: ProviderId::Gcp.as_str(),
        aliases: ProviderId::Gcp.aliases(),
        provider: Some(ProviderId::Gcp),
        implementation: "providers::gcp::inventory",
        summary: "Enumerate GCP compute, storage, IAM, network, and managed-service assets.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::Inventory(InventoryAdapter::Gcp),
        config: GCP_COMPUTE_CONFIG,
    },
    CapabilityVariant {
        id: ProviderId::Azure.as_str(),
        aliases: ProviderId::Azure.aliases(),
        provider: Some(ProviderId::Azure),
        implementation: "cli::resources::inventory",
        summary:
            "Enumerate Stado-owned Azure agent VMs; broader ARM asset inventory is not implemented.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::Inventory(InventoryAdapter::Azure),
        config: AZURE_COMPUTE_CONFIG,
    },
    CapabilityVariant {
        id: ProviderId::Aws.as_str(),
        aliases: ProviderId::Aws.aliases(),
        provider: Some(ProviderId::Aws),
        implementation: "cli::resources::inventory",
        summary:
            "Enumerate Stado-owned EC2 agent VMs; broader AWS asset inventory is not implemented.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::Inventory(InventoryAdapter::Aws),
        config: AWS_COMPUTE_CONFIG,
    },
    gpu_cloud_inventory(GpuCloudVendor::Arkane),
    gpu_cloud_inventory(GpuCloudVendor::Crusoe),
    gpu_cloud_inventory(GpuCloudVendor::Cudo),
    gpu_cloud_inventory(GpuCloudVendor::Hyperstack),
    gpu_cloud_inventory(GpuCloudVendor::Lambda),
    gpu_cloud_inventory(GpuCloudVendor::Latitude),
    gpu_cloud_inventory(GpuCloudVendor::Nebius),
    gpu_cloud_inventory(GpuCloudVendor::Oblivus),
    gpu_cloud_inventory(GpuCloudVendor::Oracle),
    gpu_cloud_inventory(GpuCloudVendor::Runpod),
    gpu_cloud_inventory(GpuCloudVendor::Salad),
    gpu_cloud_inventory(GpuCloudVendor::Scaleway),
    gpu_cloud_inventory(GpuCloudVendor::VastRental),
    gpu_cloud_inventory(GpuCloudVendor::VoltagePark),
    gpu_cloud_inventory(GpuCloudVendor::Vultr),
];

pub(in crate::capabilities::registry) const QUOTA: &[CapabilityVariant] = &[
    CapabilityVariant {
        id: ProviderId::Gcp.as_str(),
        aliases: ProviderId::Gcp.aliases(),
        provider: Some(ProviderId::Gcp),
        implementation: "scheduler::quota",
        summary: "Live GCP accelerator quota with configured reservations.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::Quota(QuotaAdapter::Gcp),
        config: GCP_COMPUTE_CONFIG,
    },
    CapabilityVariant {
        id: ProviderId::Azure.as_str(),
        aliases: ProviderId::Azure.aliases(),
        provider: Some(ProviderId::Azure),
        implementation: "scheduler::quota",
        summary: "Live Azure VM-family quota with configured reservations.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::Quota(QuotaAdapter::Azure),
        config: AZURE_COMPUTE_CONFIG,
    },
    CapabilityVariant {
        id: "storage-overlay",
        aliases: &[],
        provider: Some(ProviderId::Stado),
        implementation: "config/quotas.json",
        summary: "Provider-neutral static quota and reservation overlay.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::Quota(QuotaAdapter::StorageOverlay),
        config: &[],
    },
];

pub(in crate::capabilities::registry) const BILLING: &[CapabilityVariant] = &[
    CapabilityVariant {
        id: ProviderId::Gcp.as_str(),
        aliases: ProviderId::Gcp.aliases(),
        provider: Some(ProviderId::Gcp),
        implementation: "monitor::billing",
        summary: "GCP credits, burn, budgets and billing-health snapshot.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::Billing(BillingAdapter::Gcp),
        config: GCP_COMPUTE_CONFIG,
    },
    CapabilityVariant {
        id: ProviderId::Azure.as_str(),
        aliases: ProviderId::Azure.aliases(),
        provider: Some(ProviderId::Azure),
        implementation: "monitor::billing",
        summary: "Azure balance, usage and billing-health snapshot.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::Billing(BillingAdapter::Azure),
        config: AZURE_COMPUTE_CONFIG,
    },
];

pub(in crate::capabilities::registry) const ARTIFACTS: &[CapabilityVariant] = &[
    CapabilityVariant {
        id: "generic-v1",
        aliases: &[],
        provider: Some(ProviderId::Stado),
        implementation: "artifacts::registry + artifacts::validation",
        summary:
            "Generic manifest registration and validation for a kind with no adapter of its own.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::None,
        config: &[],
    },
    CapabilityVariant {
        id: "activation-dataset",
        aliases: &[],
        provider: Some(ProviderId::Huggingface),
        implementation: "artifacts::adapters::ActivationDatasetAdapter",
        summary: "Type-specific Hugging Face activation-dataset verification.",
        configurable: false,
        constructible: false,
        adapter: RuntimeAdapter::None,
        config: &[],
    },
];
