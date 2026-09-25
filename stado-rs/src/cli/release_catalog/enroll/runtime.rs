//! The running service's own Skarbiec consumer: exactly `runtime.grants` on
//! the vault owner, and its bearer on every host the product rolls out to.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::cli::host::{vault_token_sync, TokenSyncMode};
use crate::cli::{registry, CmdError};
use crate::release_control;

use super::super::publisher::fleet_hosts;

/// The bearer file a service's consumer is delivered as, under `~/.stado`.
fn runtime_token_file(product: &str) -> String {
    format!("{product}-skarbiec-token")
}

/// Run this Stado binary with `arguments` and return its stdout, or the
/// refusal it printed.
fn stado(arguments: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .args(arguments)
        .output()
        .map_err(|error| error.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// Grant the running service's consumer exactly `grants` on the vault owner
/// and deliver its bearer to every rollout target.
pub(super) async fn ensure_runtime_grant(
    product: &str,
    grants: &[String],
) -> Result<Value, CmdError> {
    for grant in grants {
        let well_formed = grant
            .split_once(':')
            .and_then(|(action, rest)| {
                rest.split_once('#')
                    .map(|(item, field)| (action, item, field))
            })
            .is_some_and(|(action, item, field)| {
                !action.is_empty() && !item.is_empty() && !field.is_empty()
            });
        if !well_formed {
            return Err(CmdError::click(format!(
                "{product}: runtime.grants entry {grant:?} is not action:item#field"
            )));
        }
    }
    let (owner, _) = fleet_hosts().await?;
    // The recorded grant lists capabilities as item#field:action.
    let wanted: BTreeSet<String> = grants
        .iter()
        .filter_map(|grant| grant.split_once(':'))
        .map(|(action, reference)| format!("{reference}:{action}"))
        .collect();
    let recorded: BTreeSet<String> = stado(&[
        "credentials",
        "grant",
        "show",
        "--host",
        &owner,
        product,
        "--json",
    ])
    .ok()
    .and_then(|text| serde_json::from_str::<Value>(&text).ok())
    .and_then(|record| {
        record
            .get("capabilities")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
    })
    .unwrap_or_default();
    let token_file = runtime_token_file(product);
    let minted = recorded != wanted;
    if minted {
        let capabilities = grants.join(",");
        stado(&[
            "credentials", "token", "mint", product, "--host", &owner, "--capabilities",
            &capabilities, "--audience", product, "--replace-capabilities",
            "--token-file-name", &token_file,
        ])
        .map_err(|refusal| {
            CmdError::click(format!(
                "{product}: minting its runtime bearer on {owner} with {capabilities} failed: {refusal}"
            ))
        })?;
        eprintln!("{product}: runtime consumer {product} granted {capabilities} on {owner}");
    }

    let (document, _) = registry::fetch_versioned_document().await?;
    let targets: Vec<String> = release_control::control(&document)?
        .and_then(|control| {
            control
                .products
                .get(product)
                .map(|policy| policy.targets.keys().cloned().collect())
        })
        .unwrap_or_default();
    // The copy is idempotent, and repeating it is what repairs a target that
    // missed an earlier delivery or joined the rollout later.
    let path = format!("~/.stado/{token_file}");
    let mut delivered = Vec::new();
    for target in targets.iter().filter(|target| **target != owner) {
        vault_token_sync(
            &owner,
            target,
            product,
            &path,
            &path,
            TokenSyncMode::Install,
            false,
        )
        .await?;
        delivered.push(target.clone());
    }
    Ok(json!({
        "step": "runtime-grant",
        "product": product,
        "consumer": product,
        "granted_on": owner,
        "capabilities": grants,
        "minted": minted,
        "delivered_to": delivered,
        "rollout_targets": targets,
    }))
}
