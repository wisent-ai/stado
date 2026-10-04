//! Startup/doctor validation for the dashboard object authorization boundary.
//! The verifier grant must expose exactly the mapped object items, may also
//! expose the independently diagnosed host-health route item, and every visible
//! token must be pairwise distinct so no bearer can cross a route even after an
//! accidental duplicate secret rotation.

use std::collections::{BTreeSet, HashMap};

use sha2::{Digest, Sha256};

use crate::skarbiec::{Client, SkarbiecError};

pub async fn validate_object_verifier() -> Result<usize, SkarbiecError> {
    let namespaces = crate::config::object_api_namespaces().map_err(|problems| {
        SkarbiecError::Deployment(format!(
            "invalid object_api.namespaces: {}",
            problems.join("; ")
        ))
    })?;
    // A policy that parses but leaves a queue prefix ungranted is the object
    // API answering 401 to every agent claim; it is a deployment defect of
    // this host, not a credential problem of the caller.
    if let Some(problem) = crate::config::queue_prefix_problem(namespaces) {
        return Err(SkarbiecError::Deployment(problem));
    }
    let client = Client::stado()?;
    let expected = namespaces
        .values()
        .map(|policy| policy.item().to_string())
        .collect::<BTreeSet<_>>();
    let listed = client
        .list_items()
        .await?
        .into_iter()
        .filter(|item| item.deleted != Some(true))
        .collect::<Vec<_>>();
    let visible = listed
        .iter()
        .map(|item| item.id.clone())
        .collect::<BTreeSet<_>>();
    // The host-health bearer is the item that plays its role, when this grant
    // sees one; two items in the role are a deployment fault, never a guess.
    let host_health_item =
        match crate::skarbiec::roles::holders(&listed, crate::config::HOST_HEALTH_API_ROLE)
            .as_slice()
        {
            [] => None,
            [one] => Some(one.id.clone()),
            several => {
                return Err(SkarbiecError::Deployment(format!(
                    "{} items play role {}; exactly one item may hold the host-health bearer",
                    several.len(),
                    crate::config::HOST_HEALTH_API_ROLE
                )))
            }
        };
    // This client also reads other Stado boundaries; only the declared object
    // items must be visible here. Host-health has its own authorization check.
    if !expected.is_subset(&visible) {
        let missing = expected
            .difference(&visible)
            .cloned()
            .collect::<Vec<_>>()
            .join(",");
        return Err(SkarbiecError::Deployment(format!(
            "object verifier grant is missing items [{missing}]"
        )));
    }

    let mut token_owners = HashMap::<Vec<u8>, String>::new();
    // A Skarbiec request decrypts and rewrites shared vault/audit state.
    // Unbounded startup fan-out caused intermittent connection resets and
    // made the complete authorization boundary fail closed. Read serially:
    // startup latency is bounded by the outer timeout, while the vault sees
    // one operation at a time.
    let mut scope_items = namespaces
        .iter()
        .map(|(namespace, policy)| (format!("namespace {namespace}"), policy.item()))
        .collect::<Vec<_>>();
    if let Some(item) = host_health_item.as_deref() {
        scope_items.push(("host-health route".to_string(), item));
    }
    let verified_count = scope_items.len();
    for (scope, item) in scope_items {
        let token = client
            .read_declared_string(item, "token")
            .await
            // A vault that could not answer is not a deployment verdict. The
            // context is still worth having, so it is added only to the
            // errors that describe configuration; an unavailable Skarbiec is
            // propagated with its own type intact so the caller can report
            // that nothing was measured.
            .map_err(|error| {
                if error.is_unavailable() {
                    error
                } else {
                    SkarbiecError::Deployment(format!(
                        "reading {item}/token for {scope} failed: {error}"
                    ))
                }
            })?
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                SkarbiecError::Deployment(format!(
                    "Skarbiec item {item}/token is missing or empty for {scope}"
                ))
            })?;
        let digest = Sha256::digest(token.as_bytes()).to_vec();
        if let Some(other) = token_owners.insert(digest, scope.clone()) {
            return Err(SkarbiecError::Deployment(format!(
                "dashboard bearer values for {other} and {scope} must be distinct"
            )));
        }
    }
    Ok(verified_count)
}
