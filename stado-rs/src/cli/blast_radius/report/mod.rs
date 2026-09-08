//! The human rendering of an assembled report: overall state, each store,
//! the credential store, infrastructure probes, downstream rows, and the
//! ordered recovery steps an operator follows.

mod probes;

use crate::cli::blast_radius::BlastRadiusReport;
use probes::print_probe_highlights;

pub(in crate::cli::blast_radius) fn print_human(report: &BlastRadiusReport) {
    println!("Dependency: {}", report.dependency);
    println!("Overall: {}", report.summary.state);
    println!(
        "Primary: {} ({})",
        report
            .primary_storage
            .locator
            .as_deref()
            .unwrap_or("not configured"),
        report.primary_storage.state
    );
    println!(
        "Backup: {} ({})",
        report
            .backup_storage
            .locator
            .as_deref()
            .unwrap_or("not configured"),
        report.backup_storage.state
    );
    println!("Backup coverage: {}", report.backup_coverage.state);
    println!("Scale source: {}", report.summary.scale_source);
    println!(
        "Objects in scope: primary={}, backup={}",
        optional_count(report.summary.primary_objects_in_scope),
        optional_count(report.summary.backup_objects_in_scope)
    );
    println!();
    println!(
        "Credential store: {} ({}, consumer={}, items={})",
        report.credential_store.state,
        report.credential_store.locator,
        report.credential_store.consumer,
        optional_count(report.credential_store.item_count),
    );
    if !report.credential_store.items.is_empty() {
        println!(
            "  item ids: {}",
            report
                .credential_store
                .items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if !report.credential_store.missing_required.is_empty() {
        println!(
            "  missing required items: {}",
            report.credential_store.missing_required.join(", ")
        );
    }
    if let Some(error) = &report.credential_store.error {
        println!("  error: {}", error.lines().next().unwrap_or(error));
    }
    if let Some(infrastructure) = &report.infrastructure {
        println!();
        println!(
            "GCP infrastructure: {} (checks={}, healthy={}, critical_failures={})",
            infrastructure.summary.state,
            infrastructure.summary.probes,
            infrastructure.summary.healthy,
            infrastructure.summary.critical_failures,
        );
        for probe in &infrastructure.probes {
            let count = probe
                .count
                .map_or_else(String::new, |count| format!(", count={count}"));
            println!(
                "- [{}] {} / {}: {}{} — {}",
                probe.severity, probe.service, probe.name, probe.state, count, probe.resource,
            );
            if let Some(error) = &probe.error {
                println!("  error: {}", error.lines().next().unwrap_or(error));
            }
            print_probe_highlights(probe);
        }
    }

    println!();
    println!("Downstream consumers:");
    for impact in &report.downstream {
        println!(
            "- [{}] {}: {} — {}",
            impact.severity,
            impact.component,
            impact.state,
            impact.consumers.join(", ")
        );
    }
    println!();
    println!("Automatic failover: disabled");
    println!("Safe mode: {}", report.failover.safe_mode);
    if let Some(error) = &report.primary_storage.error {
        println!("Primary error: {error}");
    }
    if let Some(error) = &report.backup_storage.error {
        println!("Backup error: {error}");
    }
    println!(
        "Machine-readable detail: stado blast-radius --dependency {} --json",
        report.dependency
    );
    println!("Recovery order:");
    for step in &report.recovery_order {
        println!("- {step}");
    }
}

fn optional_count(value: Option<usize>) -> String {
    value.map_or_else(|| "unknown".to_string(), |count| count.to_string())
}
