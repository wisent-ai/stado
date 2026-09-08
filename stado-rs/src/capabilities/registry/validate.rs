//! Static integrity audit of the product catalog, the control register and
//! the runtime registry.

use std::collections::BTreeSet;

use crate::capabilities::catalog::{
    capability_support, CapabilitySupport, CAPABILITIES, PROVIDERS,
};
use crate::capabilities::config::{ConfigField, Consumer, CONTROL_CONFIG, DECLARED_FIELDS};
use crate::capabilities::runtime::{RuntimeAdapter, RuntimeFacet};

use super::entries::REGISTRY;

fn config_binding_incomplete(field: &ConfigField) -> bool {
    field.key.is_empty() || field.env.is_empty() || field.path.is_empty()
}

fn backup_binding_incomplete(field: &ConfigField) -> bool {
    field.backup_path.is_some() != field.backup_env.is_some()
        || (field.backup_required && field.backup_path.is_none())
}

fn validate_product_catalog(problems: &mut Vec<String>) {
    let mut ids = BTreeSet::new();
    for capability in CAPABILITIES {
        if !ids.insert(capability.id) {
            problems.push(format!(
                "duplicate product capability {}",
                capability.id.as_str()
            ));
        }
        if capability.summary.trim().is_empty() {
            problems.push(format!(
                "product capability {} has no user-facing summary",
                capability.id.as_str()
            ));
        }
        if capability.providers.is_empty() {
            problems.push(format!(
                "product capability {} has no provider support entries",
                capability.id.as_str()
            ));
        }
        let mut providers = BTreeSet::new();
        for support in capability.providers {
            if !providers.insert(support.provider) {
                problems.push(format!(
                    "product capability {} declares provider {} more than once",
                    capability.id.as_str(),
                    support.provider
                ));
            }
            if matches!(
                support.support,
                CapabilitySupport::Implemented | CapabilitySupport::Partial
            ) && support.implementation.trim().is_empty()
            {
                problems.push(format!(
                    "product capability {} marks {} as {} without an implementation",
                    capability.id.as_str(),
                    support.provider,
                    support.support.as_str()
                ));
            }
            if support.note.trim().is_empty() {
                problems.push(format!(
                    "product capability {} has an unexplained {} support row",
                    capability.id.as_str(),
                    support.provider
                ));
            }
            if support.support == CapabilitySupport::Unsupported {
                problems.push(format!(
                    "product capability {} explicitly lists unsupported provider {}; omit the row instead",
                    capability.id.as_str(),
                    support.provider
                ));
            }
        }
    }
}

/// Static integrity audit for the catalog itself. This is intentionally
/// allocation-light and runs only on the operator-facing discovery path.
pub fn validate_catalog() -> Vec<String> {
    let mut problems = Vec::new();
    validate_product_catalog(&mut problems);
    // Two entries claiming one path or one environment variable would make the
    // question this catalog exists to answer -- which reader owns this key --
    // have two answers, and the resolver would silently pick the first.
    let mut control_paths = BTreeSet::new();
    let mut control_envs = BTreeSet::new();
    for field in CONTROL_CONFIG {
        if config_binding_incomplete(field) || backup_binding_incomplete(field) {
            problems.push(format!(
                "control field {} has an incomplete configuration binding",
                field.key
            ));
        }
        if !control_paths.insert(field.path) {
            problems.push(format!(
                "control field {} duplicates configuration path {}",
                field.key, field.path
            ));
        }
        if !control_envs.insert(field.env) {
            problems.push(format!(
                "control field {} duplicates environment override {}",
                field.key, field.env
            ));
        }
    }
    // Same reasoning one surface over: two entries for one declaration would
    // let `registry doctor` answer "who reads this" twice, and a `Fleet` entry
    // shadowing an `Unread` one would clear an offender by accident. An empty
    // reader string is the same silence spelled differently.
    let mut declared = BTreeSet::new();
    for field in DECLARED_FIELDS {
        if !declared.insert((field.surface.label(), field.path)) {
            problems.push(format!(
                "declared field {} on the {} surface is catalogued twice",
                field.path,
                field.surface.label()
            ));
        }
        if matches!(field.consumer, Consumer::Fleet(reader) if reader.trim().is_empty()) {
            problems.push(format!(
                "declared field {} claims a reader without naming one",
                field.path
            ));
        }
    }
    let mut kinds = BTreeSet::new();
    for capability in REGISTRY {
        if !kinds.insert(capability.kind) {
            problems.push(format!(
                "duplicate capability kind {}",
                capability.kind.as_str()
            ));
        }
        let mut names = BTreeSet::new();
        for variant in capability.variants {
            if !names.insert(variant.id) {
                problems.push(format!(
                    "{} has duplicate variant {:?}",
                    capability.kind.as_str(),
                    variant.id
                ));
            }
            for alias in variant.aliases {
                if !names.insert(alias) {
                    problems.push(format!(
                        "{} has colliding alias {:?}",
                        capability.kind.as_str(),
                        alias
                    ));
                }
            }
            if variant
                .provider
                .is_some_and(|provider| !PROVIDERS.contains(&provider))
            {
                problems.push(format!(
                    "{}.{} refers to an unregistered provider",
                    capability.kind.as_str(),
                    variant.id
                ));
            }
            if variant.constructible
                && variant.provider.is_some_and(|provider| {
                    !matches!(
                        capability_support(capability.kind.product_capability(), provider),
                        CapabilitySupport::Implemented | CapabilitySupport::Partial
                    )
                })
            {
                problems.push(format!(
                    "{}.{} is constructible but its provider does not implement product capability {}",
                    capability.kind.as_str(),
                    variant.id,
                    capability.kind.product_capability()
                ));
            }
            if let Some(provider) = variant.adapter.provider() {
                if variant.provider != Some(provider) {
                    problems.push(format!(
                        "{}.{} adapter belongs to {}, but the variant declares {:?}",
                        capability.kind.as_str(),
                        variant.id,
                        provider,
                        variant.provider
                    ));
                }
            }
            let adapter_invalid = match variant.adapter {
                RuntimeAdapter::None => {
                    runtime_backed_kind(capability.kind) && variant.constructible
                }
                adapter => !adapter_matches_kind(capability.kind, adapter),
            };
            if adapter_invalid {
                problems.push(format!(
                    "{}.{} has an incompatible runtime adapter",
                    capability.kind.as_str(),
                    variant.id
                ));
            }
            let mut field_keys = BTreeSet::new();
            for field in variant.config {
                if !field_keys.insert(field.key) {
                    problems.push(format!(
                        "{}.{} has duplicate configuration key {}",
                        capability.kind.as_str(),
                        variant.id,
                        field.key
                    ));
                }
                if config_binding_incomplete(field) {
                    problems.push(format!(
                        "{}.{} has an incomplete configuration binding",
                        capability.kind.as_str(),
                        variant.id
                    ));
                }
                if backup_binding_incomplete(field) {
                    problems.push(format!(
                        "{}.{} field {} has an incomplete backup path/env binding",
                        capability.kind.as_str(),
                        variant.id,
                        field.key
                    ));
                }
            }
        }
    }

    let mut provider_names = BTreeSet::new();
    for provider in PROVIDERS {
        if !provider_names.insert(provider.as_str()) {
            problems.push(format!("duplicate provider id {}", provider.as_str()));
        }
        for alias in provider.aliases() {
            if !provider_names.insert(alias) {
                problems.push(format!("colliding provider alias {alias:?}"));
            }
        }
    }
    problems
}

fn runtime_backed_kind(kind: RuntimeFacet) -> bool {
    matches!(
        kind,
        RuntimeFacet::Compute
            | RuntimeFacet::Storage
            | RuntimeFacet::Execution
            | RuntimeFacet::Inventory
            | RuntimeFacet::Quota
            | RuntimeFacet::Billing
            | RuntimeFacet::Dependency
    )
}

fn adapter_matches_kind(kind: RuntimeFacet, adapter: RuntimeAdapter) -> bool {
    matches!(
        (kind, adapter),
        (RuntimeFacet::Compute, RuntimeAdapter::Compute(_))
            | (RuntimeFacet::Storage, RuntimeAdapter::Storage(_))
            | (RuntimeFacet::Execution, RuntimeAdapter::Execution(_))
            | (RuntimeFacet::Inventory, RuntimeAdapter::Inventory(_))
            | (RuntimeFacet::Quota, RuntimeAdapter::Quota(_))
            | (RuntimeFacet::Billing, RuntimeAdapter::Billing(_))
            | (RuntimeFacet::Dependency, RuntimeAdapter::Dependency(_))
    )
}
