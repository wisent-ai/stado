//! Which providers this deployment enables, and the gates that opens.
//!
//! The enabled and fenced lists resolve through the catalog rather than a
//! literal list of names, and the set they produce decides two further
//! sections: the release coordinates every cloud agent needs, and the
//! control-plane and workload grants Azure dispatch needs.

use serde_json::{Map, Value};

use crate::config_file::readers::{binding_in, field_in, py_truthy};

/// The canonical providers this document enables, having judged the enabled
/// and fenced lists against each other and each provider's required keys.
pub(super) fn declared(
    root: &Map<String, Value>,
    problems: &mut Vec<String>,
) -> Vec<crate::capabilities::ProviderId> {
    let configured_providers = match field_in(root, &crate::capabilities::PROVIDERS_CONFIG)
        .filter(|value| !value.is_null())
    {
        Some(Value::Array(providers)) if providers.is_empty() => {
            problems
                .push("providers must be a non-empty list of enabled provider names".to_string());
            &[]
        }
        Some(Value::Array(providers)) => providers.as_slice(),
        Some(_) => {
            problems.push("providers must be an array of enabled provider names".to_string());
            &[]
        }
        None => &[],
    };
    let disabled_providers = match field_in(root, &crate::capabilities::DISABLED_PROVIDERS_CONFIG)
        .filter(|value| !value.is_null())
    {
        Some(Value::Array(providers)) => providers.as_slice(),
        Some(_) => {
            problems
                .push("providers_disabled must be an array of fenced provider names".to_string());
            &[]
        }
        None => &[],
    };
    let mut enabled = std::collections::BTreeSet::new();
    let mut enabled_names = std::collections::BTreeSet::new();
    for provider in configured_providers {
        let Some(name) = provider.as_str() else {
            problems.push("providers entries must be provider names".to_string());
            continue;
        };
        if !enabled_names.insert(name) {
            problems.push(format!("providers contains duplicate provider {name:?}"));
            continue;
        }
        let Some(canonical) = crate::capabilities::configurable_variant(
            crate::capabilities::RuntimeFacet::Compute,
            name,
        )
        .and_then(|variant| variant.provider) else {
            problems.push(format!("unknown provider: {provider:?}"));
            continue;
        };
        if !enabled.insert(canonical) {
            problems.push(format!(
                "providers entries {name:?} and an earlier alias identify the same provider"
            ));
        }
    }
    let mut disabled = std::collections::BTreeSet::new();
    let mut disabled_names = std::collections::BTreeSet::new();
    for provider in disabled_providers {
        let Some(name) = provider.as_str() else {
            problems.push("providers_disabled entries must be provider names".to_string());
            continue;
        };
        if !disabled_names.insert(name) {
            problems.push(format!(
                "providers_disabled contains duplicate provider {name:?}"
            ));
            continue;
        }
        let Some(canonical) = crate::capabilities::configurable_variant(
            crate::capabilities::RuntimeFacet::Compute,
            name,
        )
        .and_then(|variant| variant.provider) else {
            problems.push(format!("unknown disabled provider: {provider:?}"));
            continue;
        };
        if !disabled.insert(canonical) {
            problems.push(format!(
                "providers_disabled entries {name:?} and an earlier alias identify the same provider"
            ));
        }
    }
    for provider in enabled.intersection(&disabled) {
        problems.push(format!(
            "provider {provider:?} cannot be both enabled in providers and fenced in providers_disabled"
        ));
    }
    let active_providers = enabled.iter().copied().collect::<Vec<_>>();
    for provider in &active_providers {
        let Some(variant) = crate::capabilities::variant(
            crate::capabilities::RuntimeFacet::Compute,
            provider.as_str(),
        ) else {
            continue;
        };
        for field in variant.config.iter().filter(|field| field.required) {
            let configured = field_in(root, field).is_some_and(py_truthy)
                || binding_in(root, field.alternate_path).is_some_and(py_truthy);
            if !configured {
                problems.push(format!(
                    "{} provider needs {} (environment override {})",
                    provider, field.path, field.env
                ));
            }
        }
    }
    active_providers
}

/// A cloud agent has to be told where Stado is and exactly which release to
/// install; neither can be inferred on the VM.
pub(super) fn cloud_release_coordinates(
    root: &Map<String, Value>,
    active_providers: &[crate::capabilities::ProviderId],
    problems: &mut Vec<String>,
) {
    let cloud_agent_provider = [
        crate::capabilities::ProviderId::Gcp,
        crate::capabilities::ProviderId::Aws,
        crate::capabilities::ProviderId::Azure,
    ]
    .iter()
    .any(|provider| active_providers.contains(provider));
    if cloud_agent_provider {
        let api = field_in(root, &crate::capabilities::API_URL_CONFIG)
            .and_then(Value::as_str)
            .unwrap_or_default();
        if api.is_empty() {
            problems.push(
                "cloud agents need explicit api.url for the canonical Stado endpoint".to_string(),
            );
        } else if !api.starts_with("https://") {
            problems.push("api.url must use HTTPS".to_string());
        }
        for field in [
            &crate::capabilities::RELEASE_VERSION_CONFIG,
            &crate::capabilities::RELEASE_PLATFORM_CONFIG,
        ] {
            let value = field_in(root, field)
                .and_then(Value::as_str)
                .unwrap_or_default();
            if value.is_empty()
                || value.trim() != value
                || !value
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
            {
                problems.push(format!(
                    "{} must be an exact non-empty release coordinate containing only letters, digits, '.', '_' or '-'",
                    field.path
                ));
            }
        }
    }
}

/// Azure dispatch needs a deployment binding, a read-only control-plane grant,
/// and a workload-agent grant that is none of the ones above.
pub(super) fn azure_control_plane(
    root: &Map<String, Value>,
    active_providers: &[crate::capabilities::ProviderId],
    problems: &mut Vec<String>,
) {
    let azure_provider = active_providers.contains(&crate::capabilities::ProviderId::Azure);
    if azure_provider {
        if !field_in(root, &crate::capabilities::DEPLOYMENT_ID_CONFIG).is_some_and(py_truthy) {
            problems.push(
                "Azure control plane needs deployment.id for trusted-proxy \
                 deployment binding"
                    .to_string(),
            );
        }
        let control = crate::capabilities::SECRETS_SKARBIEC;
        for (field, remedy) in [
            (
                &control.url,
                "Azure control plane needs secrets.skarbiec.url to resolve service credentials",
            ),
            (
                &control.consumer,
                "Azure control plane needs a dedicated secrets.skarbiec.consumer",
            ),
            (
                &control.token_file,
                "Azure control plane needs an owner-only secrets.skarbiec.token_file",
            ),
        ] {
            if !field_in(root, field).is_some_and(py_truthy) {
                problems.push(remedy.to_string());
            }
        }
    }
    if azure_provider
        && field_in(root, &crate::capabilities::SECRETS_SKARBIEC.consumer).and_then(Value::as_str)
            != Some("stado-control-plane")
    {
        problems.push(
            "Azure coordinator/dashboard must use the dedicated read-only \
             secrets.skarbiec.consumer stado-control-plane"
                .to_string(),
        );
    }
    if azure_provider {
        let agent_url = field_in(root, &crate::capabilities::AGENT_SKARBIEC.url)
            .and_then(Value::as_str)
            .unwrap_or_default();
        let agent_consumer = field_in(root, &crate::capabilities::AGENT_SKARBIEC.consumer)
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !agent_url.starts_with("https://") {
            problems.push(
                "agent.skarbiec.url must be an HTTPS Skarbiec endpoint reachable from Azure VMs"
                    .to_string(),
            );
        }
        if agent_consumer.is_empty()
            || matches!(
                agent_consumer,
                "stado-control-plane" | "stado-local-agent" | "stado-azure-agent"
            )
        {
            problems.push(
                "Azure dispatch requires a newly scoped workload-agent consumer distinct from \
                 control-plane, local-agent, and revoked legacy Azure-agent grants"
                    .to_string(),
            );
        }
        if !field_in(root, &crate::capabilities::AGENT_SKARBIEC.token_file).is_some_and(py_truthy) {
            problems.push(
                "agent.skarbiec.token_file is required; Stado cannot dispatch Azure VMs \
                 without an operator-provided owner-only workload grant"
                    .to_string(),
            );
        }
        let agent_items = field_in(root, &crate::capabilities::AGENT_SKARBIEC_ITEMS_CONFIG)
            .and_then(Value::as_array)
            .filter(|items| !items.is_empty());
        if !agent_items.is_some_and(|items| {
            items.iter().all(|item| {
                item.as_str().is_some_and(|name| {
                    !name.is_empty() && !matches!(name, "stado-aws" | "stado-azure" | "stado-gcp")
                })
            })
        }) {
            problems.push(
                "agent.skarbiec.items must be a non-empty workload-only string array and must \
                 not contain cloud-provider credential items"
                    .to_string(),
            );
        }
    }
}
