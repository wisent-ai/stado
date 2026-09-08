//! The command surface: the parsed arguments and the single `run` entry point
//! that joins the independent probes into one report.

use clap::Args;

use crate::cli::CmdError;
use crate::config;
use crate::providers::gcp::inventory as gcp_inventory;
use crate::queue::copy::Endpoint;

use super::analysis::{
    compare_coverage, data_domains, dependency_owns_backend, downstream_impacts,
    validate_dependency,
};
use super::inventory::{gcp_inventory_options, inspect_credential_store, inspect_storage_bounded};
use super::report::print_human;
use super::{BlastRadiusReport, FailoverPolicy, Summary};

#[derive(Args, Debug)]
pub struct BlastRadiusArgs {
    /// Failed dependency to assess: gcp, azure, aws or local.
    #[arg(long, default_value = "gcp")]
    dependency: String,
    /// Emit the complete machine-readable report.
    #[arg(long)]
    json: bool,
}

pub async fn run(args: &BlastRadiusArgs) -> Result<(), CmdError> {
    let dependency = validate_dependency(&args.dependency)?;

    let primary_endpoint = Endpoint::configured_primary();
    let backup_endpoint = Endpoint::configured_backup();
    let inventory_options = gcp_inventory_options(&primary_endpoint, backup_endpoint.as_ref());
    let inventory_probe = async {
        if matches!(
            dependency.adapter,
            crate::capabilities::RuntimeAdapter::Dependency(
                crate::capabilities::DependencyAdapter::Gcp
            )
        ) {
            Some(gcp_inventory::inspect(inventory_options).await)
        } else {
            None
        }
    };
    let (primary, backup, infrastructure, credential_store) = tokio::join!(
        inspect_storage_bounded("primary", Some(&primary_endpoint)),
        inspect_storage_bounded("backup", backup_endpoint.as_ref()),
        inventory_probe,
        inspect_credential_store(),
    );
    let backup_matches_primary = backup_endpoint
        .as_ref()
        .is_some_and(|endpoint| endpoint.describe() == primary_endpoint.describe());
    let coverage = compare_coverage(
        &primary,
        &backup,
        backup_endpoint.is_some(),
        backup_matches_primary,
    );
    let domains = data_domains(&primary);
    let downstream = downstream_impacts(dependency, &primary.report, &backup.report);
    let affected_components = downstream
        .iter()
        .filter(|impact| impact.state != "unaffected")
        .count();
    let primary_unavailable = primary.report.state != "reachable";
    let dependency_owns_primary = dependency_owns_backend(dependency, config::wc_storage_backend());
    let infrastructure_critical = infrastructure
        .as_ref()
        .is_some_and(|report| report.summary.critical_failures != usize::default());
    let credential_store_critical = credential_store.state != "reachable";
    let state = if infrastructure_critical
        || credential_store_critical
        || (dependency_owns_primary && primary_unavailable)
    {
        "critical_outage"
    } else if dependency_owns_primary {
        "primary_at_risk"
    } else if affected_components == usize::default() {
        "unaffected"
    } else {
        "degraded"
    };
    let (scale_source, primary_scope, backup_scope) = if primary.report.object_count.is_some() {
        (
            "primary_listing".to_string(),
            primary.report.object_count,
            backup.report.object_count,
        )
    } else if backup.report.object_count.is_some() {
        (
            "backup_listing_primary_unknown".to_string(),
            None,
            backup.report.object_count,
        )
    } else {
        ("no_readable_store".to_string(), None, None)
    };

    let infrastructure_state = infrastructure
        .as_ref()
        .map(|report| report.summary.state.clone());
    let infrastructure_checks = infrastructure
        .as_ref()
        .map_or(usize::default(), |report| report.summary.probes);
    let infrastructure_failures = infrastructure.as_ref().map_or(usize::default(), |report| {
        report
            .probes
            .iter()
            .filter(|probe| probe.state != "ok")
            .count()
    });
    let credential_store_state = credential_store.state.clone();

    let report = BlastRadiusReport {
        dependency: dependency.id.to_string(),
        configured_storage_backend: config::wc_storage_backend().to_string(),
        configured_compute_providers: config::wc_providers().to_vec(),
        summary: Summary {
            state: state.to_string(),
            affected_components,
            primary_objects_in_scope: primary_scope,
            backup_objects_in_scope: backup_scope,
            scale_source,
            infrastructure_state,
            infrastructure_checks,
            infrastructure_failures,
            credential_store_state,
        },
        primary_storage: primary.report,
        backup_storage: backup.report,
        backup_coverage: coverage,
        data_domains: domains,
        downstream,
        failover: FailoverPolicy {
            automatic: false,
            safe_mode: "fence_writers_then_explicitly_promote_one_backend".to_string(),
            reason: "queue records, provider leases and compare-and-swap state are mutable; transparent redirection risks duplicate dispatch and split brain".to_string(),
        },
        infrastructure,
        credential_store,
        recovery_order: [
            "establish one readable authoritative store",
            "fence every scheduler and worker writer",
            "verify backup namespace and metadata",
            "promote the selected backend to every participant",
            "restore registry and release distribution",
            "resume coordinators and workers",
            "verify queue lifecycle and user-facing consumers",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
    };

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report)
                .map_err(|error| CmdError::click(error.to_string()))?
        );
    } else {
        print_human(&report);
    }
    Ok(())
}
