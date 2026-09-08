//! The policy document one target's registry declaration produces, and the
//! two facts a writer compares before it rewrites the host's copy.

use std::collections::BTreeSet;

use chrono::{SecondsFormat, Utc};
use serde_json::{json, Value};

use super::POLICY_SCHEMA_VERSION;
use crate::cli::CmdError;
use crate::targets::{ComputeTarget, WelesPolicy};

/// The worker's own hostname rule, transcribed from `normalizeHostname` in
/// `weles/src/worker/identity.ts`: trim, lowercase, drop trailing dots.
/// Transcribed rather than approximated because it is a comparison, and a
/// comparison the two sides perform differently is a host that matches nothing.
/// Shared with the host-side reconciler for that reason.
pub(crate) fn normalize_hostname(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .trim_end_matches('.')
        .to_string()
}

/// Every name a target answers to: the registry name first, its declared
/// `hostnames` as aliases.
///
/// The registry name leads because that is what an operator types and what the
/// rest of this binary keys on. The declared hostnames have to be there because
/// the worker matches `os.hostname()`, which on a Mac is the `.local` form —
/// a document carrying only the registry name resolves to no entry, and no
/// entry is a worker that silently refuses every action rather than an error.
///
/// Deduplicated: the loader rejects an entry that declares one identity twice,
/// and a registry that lists a target's own name under `hostnames` is common.
fn identities(target: &ComputeTarget) -> Result<(String, Vec<String>), CmdError> {
    let hostname = normalize_hostname(&target.name);
    if hostname.is_empty() {
        return Err(CmdError::click(
            "the target has no name to publish a placement policy under",
        ));
    }
    let mut aliases: Vec<String> = Vec::new();
    for declared in &target.hostnames {
        let alias = normalize_hostname(declared);
        if alias.is_empty() || alias == hostname || aliases.contains(&alias) {
            continue;
        }
        aliases.push(alias);
    }
    Ok((hostname, aliases))
}

/// The registry's action list, checked against the grammar the consumer
/// enforces (`ACTION_RE` and `parseActions`, `placement-policy.ts`).
///
/// Checked before delivery, because the loader THROWS on a list it dislikes and
/// a worker whose placement load throws claims nothing at all. A single typo in
/// the registry would otherwise become a stopped worker discovered by absence —
/// the same shape of failure this command was written to end, arriving by the
/// same route.
fn checked_actions(target: &str, weles: &WelesPolicy) -> Result<Vec<String>, CmdError> {
    let mut seen = BTreeSet::new();
    for action in &weles.actions {
        let legible = !action.is_empty()
            && action.trim() == action
            && (action == "*"
                || action.bytes().all(|byte| {
                    byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
                }));
        if !legible {
            return Err(CmdError::click(format!(
                "{target} declares the weles action {action:?}, which the worker's placement \
                 loader refuses: an action is '*', or lowercase letters, digits and \
                 underscores. A list the loader refuses is not a narrower policy — it is a \
                 worker that claims nothing"
            )));
        }
        if !seen.insert(action.as_str()) {
            return Err(CmdError::click(format!(
                "{target} declares the weles action {action:?} twice, and the worker's loader \
                 refuses a list with duplicates"
            )));
        }
    }
    if seen.contains("*") && seen.len() != usize::from(true) {
        return Err(CmdError::click(format!(
            "{target} declares the weles wildcard alongside named actions; the loader requires \
             '*' to stand alone, because a list that says both does not say which one wins"
        )));
    }
    if weles.enabled && weles.actions.is_empty() {
        return Err(CmdError::click(format!(
            "{target} declares weles.enabled with an empty action list. The worker resolves \
             that to disabled — its loader computes `enabled && actions.length > 0` — so \
             publishing it would deliver a document that says one thing and does the other. \
             Settle it in the registry first"
        )));
    }
    Ok(weles.actions.clone())
}

/// The policy document one target's registry declaration produces.
///
/// One builder for both writers. `stado route placement publish` sends
/// these bytes from the coordinator through the audited channel, and
/// [`crate::providers::local::agent::reconcile_placement_policy`] writes the
/// same bytes on the host's own disk. Two builders would be two policies for
/// one declaration — the drift this whole path exists to end — so the shape,
/// the action grammar and the identity rule are decided here exactly once.
///
/// `generation` is the registry version the declaration was read at. Every
/// caller must have one: an unstamped document is the file nobody can trace to
/// a registry read, which is the artefact being retired, and `apply_policy`
/// refuses one on arrival.
pub(crate) fn policy_document(
    target: &ComputeTarget,
    generation: &str,
    by: &str,
) -> Result<Value, CmdError> {
    let weles = target.weles.as_ref().ok_or_else(|| {
        CmdError::click(format!(
            "{} declares no `weles` block in the registry, so there is nothing to publish. \
             Declare weles.enabled and weles.actions there first: a policy invented here \
             would be the second source of truth this command exists to remove",
            target.name
        ))
    })?;
    let actions = checked_actions(&target.name, weles)?;
    let (hostname, aliases) = identities(target)?;
    Ok(json!({
        "_source": {
            "registry_generation": generation,
            "published_at": Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
            "by": by,
        },
        "schema_version": POLICY_SCHEMA_VERSION,
        "hosts": [{
            "hostname": hostname,
            "aliases": aliases,
            "enabled": weles.enabled,
            "actions": actions,
        }],
    }))
}

/// The action list inside a document [`policy_document`] built, for reports
/// that name what was written. Read back out rather than kept beside the
/// document, so a report cannot describe a list the file does not carry.
pub(crate) fn policy_actions(policy: &Value) -> Vec<String> {
    policy
        .get("hosts")
        .and_then(Value::as_array)
        .and_then(|hosts| hosts.first())
        .and_then(|host| host.get("actions"))
        .and_then(Value::as_array)
        .map(|actions| {
            actions
                .iter()
                .filter_map(|action| action.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The two facts a host-side writer compares before it rewrites the file: the
/// entry's `enabled` flag and its action list, ignoring `_source`.
///
/// `_source` carries a fresh timestamp on every build, so comparing whole
/// documents would rewrite the file on every pass and republish a policy the
/// worker is already running. What the worker acts on is exactly this pair.
pub(crate) fn policy_effect(policy: &Value) -> (bool, Vec<String>) {
    let enabled = policy
        .get("hosts")
        .and_then(Value::as_array)
        .and_then(|hosts| hosts.first())
        .and_then(|host| host.get("enabled"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    (enabled, policy_actions(policy))
}
