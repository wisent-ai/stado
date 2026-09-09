//! Inventory, quota, billing and artifact variants.

use crate::capabilities::catalog::ProviderId;
use crate::capabilities::config::variants::{
    AWS_COMPUTE_CONFIG, AZURE_COMPUTE_CONFIG, GCP_COMPUTE_CONFIG,
};
use crate::capabilities::registry::CapabilityVariant;
use crate::capabilities::runtime::{
    BillingAdapter, InventoryAdapter, QuotaAdapter, RuntimeAdapter,
};

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
