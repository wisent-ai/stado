//! Object gateway constants and the primitives its policy parser is built from.

use std::collections::BTreeSet;

use serde_json::Value;

mod endpoints;
mod policy;
mod queue;

pub use endpoints::*;
pub use policy::*;
pub use queue::*;

/// Product namespaces that must have explicit object-gateway credentials.
/// `releases` is intentionally absent: it remains on the dedicated public
/// GET-only release route.
pub const ACTIVE_OBJECT_NAMESPACES: &[&str] = &[
    "entitlements-rotator",
    "echo",
    "content-platform",
    "growth-tactics",
    "needher",
    "oko",
    "openenv",
    "probierz",
    "trading-autonomy",
    "trading-tools",
    "weles",
    "wisent-app",
    "wisent-backend",
    "wisent-images",
    "wisent-tools",
    "wisent-trade",
];

pub const OBJECT_API_VERIFIER_CONSUMER: &str = "stado-object-api-verifier";
/// Route-scoped bearer the dashboard verifies for host-health publication.
///
/// The object verifier reads this item too because the host-health endpoint is
/// served by the same dashboard process and must not fall back to the broad
/// coordinator grant.
pub const HOST_HEALTH_API_ITEM: &str = "stado-host-health-api";

pub const OBJECT_API_ACTIONS: &[&str] = &["delete", "get", "list", "put", "stat"];

fn valid_object_prefix(prefix: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    if prefix.trim() != prefix || prefix.starts_with('/') {
        return false;
    }
    let is_subtree = prefix.ends_with('/');
    let path = prefix.trim_end_matches('/');
    !path.is_empty()
        && (is_subtree || !path.contains('/'))
        && !path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        && !path.contains('\0')
        && !path.contains('\\')
}

fn object_prefixes_overlap(left: &str, right: &str) -> bool {
    left == right
        || left.is_empty()
        || right.is_empty()
        || (left.ends_with('/') && right.starts_with(left))
        || (right.ends_with('/') && left.starts_with(right))
}

fn parse_object_actions(
    value: Option<&Value>,
    location: &str,
    use_default: bool,
    problems: &mut Vec<String>,
) -> Vec<String> {
    let Some(value) = value else {
        if use_default {
            return OBJECT_API_ACTIONS
                .iter()
                .map(|action| (*action).to_string())
                .collect();
        }
        problems.push(format!("{location} is required"));
        return Vec::new();
    };
    let Value::Array(values) = value else {
        problems.push(format!("{location} must be an array of actions"));
        return Vec::new();
    };
    if values.is_empty() {
        problems.push(format!("{location} must not be empty"));
        return Vec::new();
    }
    let mut parsed = Vec::with_capacity(values.len());
    let mut seen = BTreeSet::new();
    for value in values {
        let Some(action) = value.as_str() else {
            problems.push(format!("{location} entries must be strings"));
            continue;
        };
        if !OBJECT_API_ACTIONS.contains(&action) {
            problems.push(format!("{location} contains unsupported action {action:?}"));
            continue;
        }
        if !seen.insert(action) {
            problems.push(format!("{location} contains duplicate action {action:?}"));
            continue;
        }
        parsed.push(action.to_string());
    }
    parsed
}

fn parse_legacy_object_prefixes(
    value: Option<&Value>,
    location: &str,
    problems: &mut Vec<String>,
) -> Vec<String> {
    let Some(value) = value else {
        return vec![String::new()];
    };
    let Value::Array(values) = value else {
        problems.push(format!("{location} must be an array of strings"));
        return Vec::new();
    };
    if values.is_empty() {
        problems.push(format!("{location} must not be empty"));
        return Vec::new();
    }
    let mut parsed: Vec<String> = Vec::with_capacity(values.len());
    for value in values {
        let Some(prefix) = value.as_str() else {
            problems.push(format!("{location} entries must be strings"));
            continue;
        };
        if !valid_object_prefix(prefix) {
            problems.push(format!(
                "{location} entry {prefix:?} must be empty for namespace root, a canonical top-level object key, or a canonical path ending in '/'"
            ));
            continue;
        }
        if let Some(earlier) = parsed
            .iter()
            .find(|earlier| object_prefixes_overlap(earlier, prefix))
        {
            problems.push(format!(
                "{location} contains ambiguous overlapping prefixes {earlier:?} and {prefix:?}"
            ));
            continue;
        }
        parsed.push(prefix.to_string());
    }
    parsed
}
