//! `stado release catalog withdraw-publisher`: the inverse of
//! `declare-publisher`, for a product the release catalog no longer builds.
//!
//! A declaration outlives its product when the product is retired, renamed or
//! archived; `catalog audit` then refuses on it every run. The withdrawal
//! removes `release_api.publishers.<product>` from the vault owner, this host
//! and every named API target, then reconciles each host's release verifier so
//! none keeps a grant for a publisher its table no longer names.

use serde_json::json;

use crate::cli::host::{remote_stado_output, vault_word};
use crate::cli::CmdError;

use super::{fleet_hosts, this_host};

/// Seconds one host gets to remove the key; a config write is local and quick.
const UNSET_SECONDS: u64 = 60;
/// Seconds one verifier reconciliation gets, the bound `declare-publisher`
/// gives the same repair.
const REPAIR_SECONDS: u64 = 600;

/// Withdraw `product`'s publisher declaration from the fleet.
///
/// Refuses while the release catalog still holds the product: a catalogued
/// product is built daily and its release needs the publisher. The item in
/// the vault is left in place, so a product that comes back re-declares
/// without minting a new bearer.
pub(crate) async fn withdraw_publisher(
    product: &str,
    targets: &[String],
    json_output: bool,
) -> Result<(), CmdError> {
    vault_word("product", product)?;
    let uri = super::super::catalog_uri(product);
    if crate::cli::storage::fetch_object_versioned(&uri)
        .await?
        .is_some()
    {
        return Err(CmdError::click(format!(
            "{product}: the release catalog still holds {uri}, so the daily batch builds it and \
             its releases need the publisher; withdraw a publisher only for a retired product"
        )));
    }
    let (owner, _) = fleet_hosts().await?;
    let here = this_host().await?;
    let mut hosts = vec![owner.clone(), here];
    hosts.extend(targets.iter().cloned());
    hosts.sort();
    hosts.dedup();

    let key = format!("release_api.publishers.{product}");
    let mut report = Vec::new();
    let mut failures = Vec::new();
    let mut withdrawn = 0usize;
    for host in &hosts {
        match remote_stado_output(
            host,
            &["config", "unset", &key],
            std::time::Duration::from_secs(UNSET_SECONDS),
        )
        .await
        {
            Ok(output) => {
                withdrawn += 1;
                report.push(json!({ "step": "withdraw", "host": host, "key": key, "result": output.trim() }))
            }
            Err(error) => failures.push(format!("{host}: {key} not withdrawn: {error}")),
        }
    }
    // The reconciliation reads the authoritative publisher items from the
    // vault on the machine it runs on, so it runs on the owner for every host,
    // as `declare-publisher` does for a client that holds no vault.
    for host in &hosts {
        let arguments = [
            "repair",
            "stado",
            "--step",
            "release-verifier",
            "--target",
            host.as_str(),
            "--apply",
        ];
        match remote_stado_output(
            &owner,
            &arguments,
            std::time::Duration::from_secs(REPAIR_SECONDS),
        )
        .await
        {
            Ok(_) => report.push(json!({ "step": "verifier", "host": host, "ran_on": owner })),
            Err(error) => failures.push(format!(
                "{host}: release-verifier repair (run on {owner}) failed: {error}"
            )),
        }
    }

    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({ "product": product, "steps": report, "failures": failures })
            )?
        );
    } else {
        for failure in &failures {
            eprintln!("withdraw refusal: {failure}");
        }
        println!(
            "{product}: publisher declaration withdrawn from {} of {} host(s): {}",
            withdrawn,
            hosts.len(),
            hosts.join(", ")
        );
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(CmdError::click(format!(
            "{product}: publisher withdrawal incomplete: {}",
            failures.join("; ")
        )))
    }
}
