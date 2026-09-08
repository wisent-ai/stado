//! The command entry point and the one report every source feeds.

use std::collections::BTreeSet;

use chrono::{SecondsFormat, Utc};
use serde_json::Value;

use super::human::print_human;
use super::model::{ConfigurationReport, ResourcesReport, Summary};
use super::sources::{
    inspect_billing, inspect_compute, inspect_gcp, inspect_registry, inspect_storage,
};
use crate::cli::resources::ShowArgs;
use crate::cli::CmdError;
use crate::config;
use crate::queue::copy::Endpoint;

pub async fn run(args: &ShowArgs) -> Result<(), CmdError> {
    let report = build(args).await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_human(&report);
    }
    Ok(())
}

pub(crate) async fn build(args: &ShowArgs) -> Result<ResourcesReport, CmdError> {
    let primary = Endpoint::configured_primary();
    let backup = Endpoint::configured_backup();
    let active = config::wc_providers().to_vec();
    let disabled = config::wc_disabled_providers().to_vec();
    let configured: BTreeSet<String> = active.iter().chain(&disabled).cloned().collect();
    let mut enumerable =
        crate::capabilities::provider_ids(crate::capabilities::RuntimeFacet::Inventory)
            .into_iter()
            .map(|provider| provider.as_str().to_string())
            .filter(|provider| configured.contains(provider))
            .collect::<Vec<_>>();
    if let Some(requested) = args.provider.as_deref() {
        let provider = crate::capabilities::canonical_id(
            crate::capabilities::RuntimeFacet::Inventory,
            requested,
        )
        .ok_or_else(|| {
            CmdError::usage(format!(
                "provider {requested:?} has no inventory capability"
            ))
        })?;
        if !enumerable.iter().any(|name| name == provider) {
            return Err(CmdError::usage(format!(
                "provider {provider:?} is not a configured enumerable cloud"
            )));
        }
        enumerable.retain(|name| name == provider);
    }

    let storage_future = inspect_storage(&primary, backup.as_ref());
    let compute_future = inspect_compute(&enumerable, &active, &disabled, &primary);
    let registry_future = inspect_registry();
    let gcp_future = inspect_gcp(
        configured.contains(crate::capabilities::ProviderId::Gcp.as_str()),
        &primary,
        backup.as_ref(),
    );
    let billing_future = inspect_billing(&configured);
    let (storage, compute, host_registry, gcp_inventory, billing) = tokio::join!(
        storage_future,
        compute_future,
        registry_future,
        gcp_future,
        billing_future,
    );

    let coverage_gaps = coverage_gaps(&configured);
    let incomplete_sources = [&storage, &compute, &host_registry, &gcp_inventory, &billing]
        .into_iter()
        .filter(|source| !matches!(source.state.as_str(), "ok" | "skipped"))
        .count()
        .saturating_add(usize::from(!coverage_gaps.is_empty()));
    let visible_instances = compute
        .data
        .get("providers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|provider| provider.get("instances").and_then(Value::as_array))
        .map(Vec::len)
        .sum();
    let confirmed_orphan_instances = compute
        .data
        .get("providers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|provider| provider.get("orphan_count").and_then(Value::as_u64))
        .map(|count| count as usize)
        .sum();
    let storage_objects = storage
        .data
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("object_count").and_then(Value::as_u64))
        .map(|count| count as usize)
        .sum();
    let summary = Summary {
        state: if incomplete_sources == usize::default() {
            "complete"
        } else {
            "incomplete"
        },
        configured_providers: configured.len(),
        visible_instances,
        confirmed_orphan_instances,
        storage_objects,
        incomplete_sources,
    };
    let report = ResourcesReport {
        schema_version: u8::from(true),
        generated_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        read_only: true,
        configuration: ConfigurationReport {
            active_compute: active,
            disabled_compute: disabled,
            primary_storage: primary.describe(),
            backup_storage: backup.as_ref().map(Endpoint::describe),
        },
        summary,
        storage,
        compute,
        host_registry,
        gcp_inventory,
        billing,
        coverage_gaps,
    };

    Ok(report)
}

fn coverage_gaps(configured: &BTreeSet<String>) -> Vec<String> {
    configured
        .iter()
        .filter_map(|name| crate::capabilities::provider(name))
        .filter_map(crate::capabilities::ProviderId::inventory_limitation)
        .map(str::to_string)
        .collect()
}
