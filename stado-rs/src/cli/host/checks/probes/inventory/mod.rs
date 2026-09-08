//! `stado host inventory` — what is installed on this host, and whether
//! it matches what the registry declares.

pub(in crate::cli::host) mod binaries;
pub(in crate::cli::host) mod network;
pub(in crate::cli::host) mod reconciliation;
pub(in crate::cli::host) mod vault_tables;

use serde_json::Value;

use crate::cli::CmdError;

use crate::cli::host::checks::probes::{cell, print_json, report_outcome};

pub async fn inventory(target: &str, json: bool) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let report = crate::deploy::host_inventory::inventory_host(target, &runner)
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    let expected = crate::deploy::host_inventory::OK_STATUS;
    if json {
        print_json(&report);
        return report_outcome(&report, expected);
    }
    let section = |key: &str| {
        report
            .get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };

    println!("target:   {}", cell(report.get("target")));
    // Said before the tables, not after them, because it decides whether the
    // strings in those tables mean anything. A host whose sanitizer does not
    // work reports blank names, and a table of blanks reads like a host with
    // nothing installed on it.
    let sanitizer = report.get("sanitizer_state");
    if sanitizer.and_then(Value::as_str) != Some(crate::deploy::host_inventory::SANITIZER_OK) {
        println!(
            "sanitizer: {} — the host's own field sanitizer failed its probe, so \
             every name, mode, version and URL below is unreliable",
            cell(sanitizer)
        );
    }

    binaries::print_binaries(&report, &section);
    network::print_network(&report, &section, target);
    let (vaults, sidecars) = vault_tables::print_vaults(&report, &section);
    reconciliation::print_reconciliation(&report, &vaults, &sidecars);
    report_outcome(&report, expected)
}
