//! The queue's own object namespace and the policies the gateway accepts.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use super::HOST_HEALTH_API_ITEM;
use crate::config::{parse_object_api_namespaces, ObjectApiNamespace};
use serde_json::Value;

/// The namespace the queue lives in; every prefix in
/// [`crate::queue::copy::CANONICAL_PREFIXES`] is read and written there.
pub const QUEUE_OBJECT_NAMESPACE: &str = "probierz";

/// The five actions the queue performs on its own prefixes.
const QUEUE_OBJECT_ACTIONS: [&str; 5] = ["get", "put", "list", "stat", "delete"];

/// Canonical queue prefixes the `probierz` object policy does not grant for
/// every queue action, sorted; empty when the policy covers the queue.
///
/// A binary that reads a prefix nobody granted does not fail at the line
/// that reads it: the object API answers 401, the agent logs "agent loop
/// failed" and restarts, and the host claims nothing while its capacity
/// broadcast keeps saying it is alive. The declaration lives in each object
/// API host's config and the consumer lives in the binary, so the check
/// belongs where the two meet: `stado config validate` and `config set` on
/// the host, and `stado doctor` there.
pub fn queue_prefixes_missing(
    namespaces: &BTreeMap<String, ObjectApiNamespace>,
) -> Vec<&'static str> {
    let Some(policy) = namespaces.get(QUEUE_OBJECT_NAMESPACE) else {
        return Vec::new();
    };
    let mut missing: Vec<&'static str> = crate::queue::copy::CANONICAL_PREFIXES
        .iter()
        .copied()
        .filter(|prefix| {
            // A prefix is probed with a key under it; a root object is
            // probed as itself.
            let key = if prefix.ends_with('/') {
                format!("{prefix}probe")
            } else {
                (*prefix).to_string()
            };
            !QUEUE_OBJECT_ACTIONS
                .iter()
                .all(|action| policy.allows_object_action(&key, action))
        })
        .collect();
    missing.sort_unstable();
    missing
}

/// One sentence for a policy that leaves the queue's prefixes ungranted,
/// naming them and the command that grants them; `None` when it covers them.
pub fn queue_prefix_problem(namespaces: &BTreeMap<String, ObjectApiNamespace>) -> Option<String> {
    let missing = queue_prefixes_missing(namespaces);
    if missing.is_empty() {
        return None;
    }
    Some(format!(
        "object_api.namespaces.{QUEUE_OBJECT_NAMESPACE} does not grant the queue prefix(es) {} for \
         get, put, list, stat and delete; every agent claim against this object API answers 401. \
         Add each as a prefix_policies entry: stado host config-set <target> \
         object_api.namespaces.{QUEUE_OBJECT_NAMESPACE} '<json>' --reload-service <object-api unit>",
        missing.join(", ")
    ))
}

static OBJECT_API_NAMESPACES: LazyLock<Result<BTreeMap<String, ObjectApiNamespace>, Vec<String>>> =
    LazyLock::new(|| {
        let configured = match std::env::var("WC_OBJECT_API_NAMESPACES")
            .ok()
            .filter(|value| !value.trim().is_empty())
        {
            Some(encoded) => match serde_json::from_str::<Value>(&encoded) {
                Ok(value) => Some(value),
                Err(error) => {
                    return Err(vec![format!(
                        "WC_OBJECT_API_NAMESPACES must be a JSON object: {error}"
                    )])
                }
            },
            None => crate::config_file::get("object_api.namespaces"),
        };
        parse_object_api_namespaces(configured.as_ref())
    });

/// Exact private product namespace policies accepted by the object gateway.
pub fn object_api_namespaces(
) -> Result<&'static BTreeMap<String, ObjectApiNamespace>, &'static [String]> {
    match &*OBJECT_API_NAMESPACES {
        Ok(namespaces) => Ok(namespaces),
        Err(problems) => Err(problems.as_slice()),
    }
}
/// Exact item set visible to the dashboard's least-privilege verifier.
///
/// Kept in one function because startup validation and remote reconciliation
/// must agree byte-for-byte: adding a protected route to only one side closes
/// the whole boundary as either missing or over-broad.
pub fn object_verifier_items(
    namespaces: &BTreeMap<String, ObjectApiNamespace>,
) -> BTreeSet<String> {
    namespaces
        .values()
        .map(|policy| policy.item().to_string())
        .chain(std::iter::once(HOST_HEALTH_API_ITEM.to_string()))
        .collect()
}

/// Policy for one canonical namespace. Invalid aggregate configuration fails
/// closed for every namespace rather than partially enabling the valid rows.
pub fn object_api_namespace(namespace: &str) -> Option<&'static ObjectApiNamespace> {
    object_api_namespaces().ok()?.get(namespace)
}
