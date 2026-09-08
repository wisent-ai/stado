//! The runtime registry: one row per facet, and every lookup over it.

use crate::capabilities::catalog::{capability_support, CapabilitySupport, ProviderId};
use crate::capabilities::config::ConfigField;
use crate::capabilities::runtime::{
    ExecutionAdapter, RuntimeAdapter, RuntimeFacet, SelectionMode, StorageAdapter,
};

use super::families::compute::{COMPUTE, EXECUTION, SCHEDULING, STORAGE};
use super::families::delivery::{DEPENDENCY, DEPLOYMENT, HOST_TARGET};
use super::families::fleet::{ARTIFACTS, BILLING, INVENTORY, QUOTA};
use super::families::platform::{ALERTS, AUTHENTICATION, SECRETS};
use super::types::{Capability, CapabilityVariant};

pub static REGISTRY: &[Capability] = &[
    Capability {
        kind: RuntimeFacet::Compute,
        selection: SelectionMode::OrderedMany,
        summary: "Machine provisioning and lifecycle management.",
        variants: COMPUTE,
    },
    Capability {
        kind: RuntimeFacet::Storage,
        selection: SelectionMode::Single,
        summary: "Queue state, control data and provider-neutral product objects.",
        variants: STORAGE,
    },
    Capability {
        kind: RuntimeFacet::Execution,
        selection: SelectionMode::Automatic,
        summary: "Job execution inside long-lived or ephemeral agents.",
        variants: EXECUTION,
    },
    Capability {
        kind: RuntimeFacet::Scheduling,
        selection: SelectionMode::Internal,
        summary: "Provider-neutral assignment, dispatch and recurring schedules.",
        variants: SCHEDULING,
    },
    Capability {
        kind: RuntimeFacet::Inventory,
        selection: SelectionMode::ConcurrentMany,
        summary: "Provider-owned asset discovery.",
        variants: INVENTORY,
    },
    Capability {
        kind: RuntimeFacet::Quota,
        selection: SelectionMode::Automatic,
        summary: "Cloud quota discovery and configured capacity reservations.",
        variants: QUOTA,
    },
    Capability {
        kind: RuntimeFacet::Billing,
        selection: SelectionMode::ConcurrentMany,
        summary: "Cloud balances, budgets, burn and billing health.",
        variants: BILLING,
    },
    Capability {
        kind: RuntimeFacet::Artifacts,
        selection: SelectionMode::Automatic,
        summary: "Artifact manifests, registry and type-specific verification.",
        variants: ARTIFACTS,
    },
    Capability {
        kind: RuntimeFacet::Authentication,
        selection: SelectionMode::Automatic,
        summary: "Object, release, machine, service and host-health request authorization.",
        variants: AUTHENTICATION,
    },
    Capability {
        kind: RuntimeFacet::Secrets,
        selection: SelectionMode::Automatic,
        summary: "Application secrets and cloud workload identity.",
        variants: SECRETS,
    },
    Capability {
        kind: RuntimeFacet::Alerts,
        selection: SelectionMode::ConcurrentMany,
        summary: "Fault-isolated operator alert delivery.",
        variants: ALERTS,
    },
    Capability {
        kind: RuntimeFacet::Deployment,
        selection: SelectionMode::Automatic,
        summary: "Service installation, VM bootstrap and binary releases.",
        variants: DEPLOYMENT,
    },
    Capability {
        kind: RuntimeFacet::HostTarget,
        selection: SelectionMode::OrderedMany,
        summary: "Host registry target kinds.",
        variants: HOST_TARGET,
    },
    Capability {
        kind: RuntimeFacet::Dependency,
        selection: SelectionMode::Single,
        summary: "Blast-radius dependency ownership and inspection.",
        variants: DEPENDENCY,
    },
];

pub fn all() -> &'static [Capability] {
    REGISTRY
}

pub fn get(id: &str) -> Option<&'static Capability> {
    REGISTRY
        .iter()
        .find(|capability| capability.kind.as_str() == id)
}

pub fn variant(kind: RuntimeFacet, name: &str) -> Option<&'static CapabilityVariant> {
    let capability = REGISTRY.iter().find(|entry| entry.kind == kind)?;
    capability
        .variants
        .iter()
        .find(|entry| entry.id == name || entry.aliases.contains(&name))
}

pub fn storage_adapter(name: &str) -> Option<StorageAdapter> {
    match variant(RuntimeFacet::Storage, name).map(|variant| variant.adapter) {
        Some(RuntimeAdapter::Storage(adapter)) => Some(adapter),
        _ => None,
    }
}

pub fn execution_adapter(name: &str) -> Option<ExecutionAdapter> {
    match variant(RuntimeFacet::Execution, name).map(|variant| variant.adapter) {
        Some(RuntimeAdapter::Execution(adapter)) => Some(adapter),
        _ => None,
    }
}

pub fn configurable_variant(kind: RuntimeFacet, name: &str) -> Option<&'static CapabilityVariant> {
    variant(kind, name).filter(|entry| entry.configurable)
}

pub fn constructible_variant(kind: RuntimeFacet, name: &str) -> Option<&'static CapabilityVariant> {
    variant(kind, name).filter(|entry| {
        if !entry.constructible {
            return false;
        }
        entry.provider.is_none_or(|provider| {
            matches!(
                capability_support(kind.product_capability(), provider),
                CapabilitySupport::Implemented | CapabilitySupport::Partial
            )
        })
    })
}

pub fn configurable_ids(kind: RuntimeFacet) -> impl Iterator<Item = &'static str> {
    REGISTRY
        .iter()
        .find(|entry| entry.kind == kind)
        .into_iter()
        .flat_map(|entry| entry.variants)
        .filter(|variant| variant.configurable)
        .map(|variant| variant.id)
}

pub fn config_fields(kind: RuntimeFacet) -> impl Iterator<Item = &'static ConfigField> {
    REGISTRY
        .iter()
        .find(|entry| entry.kind == kind)
        .into_iter()
        .flat_map(|entry| entry.variants)
        .flat_map(|variant| variant.config)
}

pub fn config_field(
    kind: RuntimeFacet,
    variant_name: &str,
    key: &str,
) -> Option<&'static ConfigField> {
    variant(kind, variant_name)?
        .config
        .iter()
        .find(|field| field.key == key)
}

pub fn config_envs(kind: RuntimeFacet) -> impl Iterator<Item = &'static str> {
    config_fields(kind)
        .flat_map(|field| [Some(field.env), field.backup_env])
        .flatten()
}

pub fn backup_config_envs(kind: RuntimeFacet) -> impl Iterator<Item = &'static str> {
    config_fields(kind).filter_map(|field| field.backup_env)
}

pub fn config_env(kind: RuntimeFacet, variant_name: &str, key: &str) -> Option<&'static str> {
    config_field(kind, variant_name, key).map(|field| field.env)
}

pub fn same_variant(kind: RuntimeFacet, left: &str, right: &str) -> bool {
    match (variant(kind, left), variant(kind, right)) {
        (Some(left), Some(right)) => left.id == right.id,
        _ => left == right,
    }
}

pub fn provider_ids(kind: RuntimeFacet) -> Vec<ProviderId> {
    let mut providers = Vec::new();
    if let Some(capability) = REGISTRY.iter().find(|entry| entry.kind == kind) {
        for provider in capability
            .variants
            .iter()
            .filter_map(|variant| variant.provider)
        {
            if !providers.contains(&provider) {
                providers.push(provider);
            }
        }
    }
    providers
}

pub fn canonical_id(kind: RuntimeFacet, name: &str) -> Option<&'static str> {
    variant(kind, name).map(|variant| variant.id)
}
