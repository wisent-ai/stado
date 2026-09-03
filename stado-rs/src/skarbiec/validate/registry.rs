//! Startup validation of the registry-policy verifier boundary.
//!
//! The boundary this validates gates `/api/registry.json`,
//! `/api/registry/policy`, `/api/cleanup.json` and `/api/cleanup/run` — the
//! four routes Stado Desktop calls to read a fleet's cleanup policy, rewrite
//! one target's whitelisted fields, read the janitor's last report and ask for
//! a pass.
//!
//! One judgement separates this validator from its siblings: an UNDECLARED
//! boundary is valid. `registry_api.clients` absent or empty means no client
//! has been granted these routes, so every request to them is refused with
//! `401` — a refusal, not an outage — and no grant file is expected to exist
//! yet. Requiring a grant here would report `registry authorization
//! unavailable` on every deployment that has not yet declared a client, which
//! is the failure mode this codebase keeps meeting: an operator sent to look
//! for broken infrastructure by a check that was really reporting silence.
//!
//! A mapping that EXISTS is validated exactly as the other boundaries are: the
//! grant must see precisely the declared items, no more and no fewer.

use std::collections::BTreeSet;

use super::{Client, SkarbiecError};

/// Number of registry-API client items the grant is expected to see.
pub async fn validate_registry_verifier() -> Result<usize, SkarbiecError> {
    let clients = crate::config::registry_api_clients().map_err(|problems| {
        SkarbiecError::Deployment(format!(
            "invalid registry_api.clients: {}",
            problems.join("; ")
        ))
    })?;
    if clients.is_empty() {
        return Ok(0);
    }
    let client = Client::registry_verifier()?;
    let expected = clients
        .values()
        .map(|policy| policy.item().to_string())
        .collect::<BTreeSet<_>>();
    let visible = client
        .list_items()
        .await?
        .into_iter()
        .filter(|item| item.deleted != Some(true))
        .map(|item| item.id)
        .collect::<BTreeSet<_>>();
    if visible != expected {
        let missing = expected
            .difference(&visible)
            .cloned()
            .collect::<Vec<_>>()
            .join(",");
        let unexpected = visible
            .difference(&expected)
            .cloned()
            .collect::<Vec<_>>()
            .join(",");
        return Err(SkarbiecError::Deployment(format!(
            "registry verifier grant item set mismatch (missing=[{missing}], unexpected=[{unexpected}])"
        )));
    }
    Ok(expected.len())
}
