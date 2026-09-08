//! Object, release and service verifier grants: what config declares, and
//! the isolated bearer each consumer reads.

pub(in crate::cli::host) mod reconcile;
pub(in crate::cli::host) mod release;

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::cli::CmdError;

use crate::cli::host::checks::recovery::verifier::reconcile::reconcile_verifier;
use crate::cli::host::machine::config::remote::{remote_config_output, RemoteConfigAction};

fn object_namespace_items(document: &Value) -> Result<BTreeMap<String, String>, CmdError> {
    let namespaces = document
        .pointer("/resolved/object_api_namespaces")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            CmdError::click(
                "object_verifier_reconcile_host_declaration_unreadable: remote config has no resolved object_api_namespaces",
            )
        })?;
    namespaces
        .iter()
        .map(|(namespace, declaration)| {
            let item = declaration
                .get("item")
                .and_then(Value::as_str)
                .filter(|item| !item.is_empty())
                .ok_or_else(|| {
                    CmdError::click(format!(
                        "object_verifier_reconcile_host_declaration_unreadable: namespace {namespace:?} has no item"
                    ))
                })?;
            Ok((namespace.clone(), item.to_string()))
        })
        .collect()
}

fn ensure_object_verifier_declarations_match(
    host: &BTreeMap<String, String>,
    local_items: &BTreeSet<String>,
) -> Result<(), CmdError> {
    let local = local_items
        .iter()
        .filter(|item| item.as_str() != crate::config::HOST_HEALTH_API_ITEM)
        .cloned()
        .collect::<BTreeSet<_>>();
    let host_items = host.values().cloned().collect::<BTreeSet<_>>();
    let missing = host
        .iter()
        .filter(|(_, item)| !local.contains(item.as_str()))
        .map(|(namespace, item)| format!("{namespace}={item}"))
        .collect::<Vec<_>>();
    let unexpected = local.difference(&host_items).cloned().collect::<Vec<_>>();
    if missing.is_empty() && unexpected.is_empty() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "object_verifier_reconcile_declaration_mismatch: local object_api.namespaces cannot \
         prove TARGET's declaration (missing_local=[{}], unexpected_local_items=[{}]); copy \
         the host's exact namespace declarations locally before reconciling",
        missing.join(","),
        unexpected.join(",")
    )))
}

pub(crate) async fn apply_object_verifier_repair(target: &str) -> Result<Value, CmdError> {
    let namespaces = crate::config::object_api_namespaces().map_err(|problems| {
        CmdError::click(format!(
            "invalid object_api.namespaces: {}",
            problems.join("; ")
        ))
    })?;
    let items = crate::config::object_verifier_items(namespaces);
    // Reconciliation used to derive "exact" solely from this machine's
    // declaration. On 2026-09-04 the target declared `spis-crawls`, this
    // machine did not, and the command removed nothing missing locally before
    // reporting exact=true while the target's whole object boundary stayed
    // closed. Read the configuration the target's services actually consume
    // and refuse before touching its grant when the two inputs differ.
    let canonical = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let stdout = remote_config_output(
        &canonical,
        RemoteConfigAction::Show,
        &crate::deploy::production_runner(),
    )
    .await?;
    let document: Value = serde_json::from_str(&stdout).map_err(|error| {
        CmdError::click(format!(
            "object_verifier_reconcile_host_declaration_unreadable: {error}"
        ))
    })?;
    let host = object_namespace_items(&document)?;
    ensure_object_verifier_declarations_match(&host, &items)?;
    reconcile_verifier(
        target,
        "object",
        "matching local and target object_api.namespaces plus the host-health route",
        crate::config::OBJECT_API_VERIFIER_CONSUMER,
        "WC_OBJECT_SKARBIEC_TOKEN_FILE",
        "stado-object-api-verifier-skarbiec-token",
        items,
        true,
    )
    .await
}

fn release_publisher_items(document: &Value) -> Result<BTreeMap<String, String>, CmdError> {
    let publishers = document
        .pointer("/resolved/release_api_publishers")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            CmdError::click(
                "release_verifier_reconcile_host_declaration_unreadable: remote config has no resolved release_api_publishers",
            )
        })?;
    publishers
        .iter()
        .map(|(product, declaration)| {
            let item = declaration
                .get("item")
                .and_then(Value::as_str)
                .filter(|item| !item.is_empty())
                .ok_or_else(|| {
                    CmdError::click(format!(
                        "release_verifier_reconcile_host_declaration_unreadable: product {product:?} has no item"
                    ))
                })?;
            Ok((product.clone(), item.to_string()))
        })
        .collect()
}
