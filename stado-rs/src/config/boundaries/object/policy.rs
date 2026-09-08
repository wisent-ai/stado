//! Object namespace policy: one product credential and its key boundaries.

use std::collections::{BTreeMap, BTreeSet};

use super::{
    object_prefixes_overlap, parse_legacy_object_prefixes, parse_object_actions,
    valid_object_prefix, ACTIVE_OBJECT_NAMESPACES,
};
use serde_json::Value;

/// One exact object-key boundary and the actions granted inside it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectPrefixPolicy {
    prefix: String,
    actions: Vec<String>,
}

impl ObjectPrefixPolicy {
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    pub fn actions(&self) -> &[String] {
        &self.actions
    }

    fn allows_action(&self, action: &str) -> bool {
        self.actions.iter().any(|allowed| allowed == action)
    }

    fn contains_key(&self, key: &str) -> bool {
        if self.prefix.is_empty() {
            true
        } else if self.prefix.ends_with('/') {
            key.starts_with(&self.prefix)
        } else {
            key == self.prefix
        }
    }
}

/// One product credential and its least-privilege key/action boundaries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectApiNamespace {
    item: String,
    prefix_policies: Vec<ObjectPrefixPolicy>,
}

impl ObjectApiNamespace {
    pub fn item(&self) -> &str {
        &self.item
    }

    pub fn prefix_policies(&self) -> &[ObjectPrefixPolicy] {
        &self.prefix_policies
    }

    /// Whether one canonical object key and action are granted together.
    pub fn allows_object_action(&self, key: &str, action: &str) -> bool {
        self.prefix_policies
            .iter()
            .any(|policy| policy.allows_action(action) && policy.contains_key(key))
    }

    /// Authorize and canonicalize a list prefix under one policy that grants
    /// list. Exact-key policies can never authorize a prefix scan.
    ///
    /// The caller's trailing `/` survives: authorization compares paths, and
    /// the string this returns is what the store will scan. A policy with an
    /// empty prefix grants the namespace, and returning a trimmed `queue` for
    /// a requested `queue/` is what let a listing reach `queue_priority/`.
    pub fn authorized_list_prefix(&self, requested: &str, action: &str) -> Option<String> {
        let requested = requested.trim_start_matches('/');
        let path = requested.trim_end_matches('/');
        self.prefix_policies.iter().find_map(|policy| {
            if !policy.allows_action(action) {
                return None;
            }
            let allowed = policy.prefix();
            if allowed.is_empty() {
                return Some(requested.to_string());
            }
            if !allowed.ends_with('/') {
                return None;
            }
            let root = allowed.strip_suffix('/').unwrap_or(allowed);
            if path == root {
                Some(allowed.to_string())
            } else if path.starts_with(allowed) {
                Some(requested.to_string())
            } else {
                None
            }
        })
    }
}

/// Parse the security-sensitive namespace map without applying defaults.
/// Each item name is bound to its namespace by construction, preventing a
/// typo from granting product A the bearer belonging to product B.
pub(crate) fn parse_object_api_namespaces(
    value: Option<&Value>,
) -> Result<BTreeMap<String, ObjectApiNamespace>, Vec<String>> {
    let Some(Value::Object(entries)) = value else {
        return Err(vec![
            "object_api.namespaces must be a non-empty object mapping namespaces to Skarbiec items"
                .to_string(),
        ]);
    };
    if entries.is_empty() {
        return Err(vec![
            "object_api.namespaces must not be empty; product object routes fail closed without an explicit mapping"
                .to_string(),
        ]);
    }

    let mut problems = Vec::new();
    let mut namespaces = BTreeMap::new();
    let mut items = BTreeSet::new();
    for (namespace, raw_entry) in entries {
        let problem_count = problems.len();
        if namespace.trim() != namespace
            || namespace == "releases"
            || crate::object_store::ObjectRef::new(namespace, "sentinel").is_err()
        {
            problems.push(format!(
                "object_api.namespaces key {namespace:?} is not a canonical private product namespace"
            ));
        }
        let Some(entry) = raw_entry.as_object() else {
            problems.push(format!(
                "object_api.namespaces.{namespace} must be an object with item and either prefix_policies or legacy prefixes/actions"
            ));
            continue;
        };
        for key in entry.keys() {
            if !matches!(
                key.as_str(),
                "item" | "prefixes" | "actions" | "prefix_policies"
            ) {
                problems.push(format!(
                    "object_api.namespaces.{namespace} contains unsupported key {key:?}"
                ));
            }
        }
        let item = entry
            .get("item")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let expected_item = if namespace == "wisent-backend" {
            "wisent-backend-object-client".to_string()
        } else {
            format!("{namespace}-object-api")
        };
        if item != expected_item {
            problems.push(format!(
                "object_api.namespaces.{namespace}.item must be {expected_item:?}, got {item:?}"
            ));
        }

        let mut prefix_policies = Vec::new();
        if let Some(explicit) = entry.get("prefix_policies") {
            if entry.contains_key("prefixes") || entry.contains_key("actions") {
                problems.push(format!(
                    "object_api.namespaces.{namespace} cannot combine prefix_policies with legacy prefixes/actions"
                ));
            }
            match explicit {
                Value::Array(values) if !values.is_empty() => {
                    for (index, raw_policy) in values.iter().enumerate() {
                        let location =
                            format!("object_api.namespaces.{namespace}.prefix_policies[{index}]");
                        let Some(policy) = raw_policy.as_object() else {
                            problems.push(format!(
                                "{location} must be an object with exact prefix and actions"
                            ));
                            continue;
                        };
                        for key in policy.keys() {
                            if !matches!(key.as_str(), "prefix" | "actions") {
                                problems
                                    .push(format!("{location} contains unsupported key {key:?}"));
                            }
                        }
                        let prefix = policy
                            .get("prefix")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if prefix.is_empty() || !valid_object_prefix(prefix) {
                            problems.push(format!(
                                "{location}.prefix must be a non-empty canonical top-level object key or path ending in '/'"
                            ));
                        }
                        let actions = parse_object_actions(
                            policy.get("actions"),
                            &format!("{location}.actions"),
                            false,
                            &mut problems,
                        );
                        if let Some(earlier) =
                            prefix_policies
                                .iter()
                                .find(|earlier: &&ObjectPrefixPolicy| {
                                    object_prefixes_overlap(earlier.prefix(), prefix)
                                })
                        {
                            problems.push(format!(
                                "{location}.prefix {prefix:?} ambiguously overlaps earlier prefix {:?}",
                                earlier.prefix()
                            ));
                        }
                        prefix_policies.push(ObjectPrefixPolicy {
                            prefix: prefix.to_string(),
                            actions,
                        });
                    }
                }
                Value::Array(_) => problems.push(format!(
                    "object_api.namespaces.{namespace}.prefix_policies must not be empty"
                )),
                _ => problems.push(format!(
                    "object_api.namespaces.{namespace}.prefix_policies must be an array"
                )),
            }
        } else {
            let prefixes = parse_legacy_object_prefixes(
                entry.get("prefixes"),
                &format!("object_api.namespaces.{namespace}.prefixes"),
                &mut problems,
            );
            let actions = parse_object_actions(
                entry.get("actions"),
                &format!("object_api.namespaces.{namespace}.actions"),
                true,
                &mut problems,
            );
            prefix_policies.extend(prefixes.into_iter().map(|prefix| ObjectPrefixPolicy {
                prefix,
                actions: actions.clone(),
            }));
        }
        if !items.insert(item.to_string()) {
            problems.push(format!(
                "object_api.namespaces maps more than one namespace to item {item:?}"
            ));
        }
        if problems.len() == problem_count {
            namespaces.insert(
                namespace.to_string(),
                ObjectApiNamespace {
                    item: item.to_string(),
                    prefix_policies,
                },
            );
        }
    }
    for &required in ACTIVE_OBJECT_NAMESPACES {
        if !namespaces.contains_key(required) {
            problems.push(format!(
                "object_api.namespaces is missing active namespace {required:?}"
            ));
        }
    }
    if problems.is_empty() {
        Ok(namespaces)
    } else {
        Err(problems)
    }
}
