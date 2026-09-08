//! The vault stretch of `config show`: the four skarbiec audiences this
//! deployment reads and writes through — object, release, service and agent —
//! each with its endpoint, its consumer, its token file and the policy map
//! that says what it may touch. A policy map that will not resolve is
//! reported in place, under an `errors` key, rather than failing the command:
//! `config show` is what an operator runs to find out why.

use serde_json::{Map, Value};

use crate::config;

pub(super) fn insert(resolved: &mut Map<String, Value>) {
    resolved.insert(
        "object_skarbiec_url".into(),
        Value::from(config::object_skarbiec_url()),
    );
    resolved.insert(
        "object_skarbiec_consumer".into(),
        Value::from(config::object_skarbiec_consumer()),
    );
    resolved.insert(
        "object_skarbiec_token_file".into(),
        Value::from(config::object_skarbiec_token_file()),
    );
    let object_namespaces = match config::object_api_namespaces() {
        Ok(namespaces) => Value::Object(
            namespaces
                .iter()
                .map(|(namespace, policy)| {
                    (
                        namespace.clone(),
                        Value::Object(Map::from_iter([
                            ("item".into(), Value::from(policy.item())),
                            (
                                "prefix_policies".into(),
                                Value::Array(
                                    policy
                                        .prefix_policies()
                                        .iter()
                                        .map(|prefix_policy| {
                                            Value::Object(Map::from_iter([
                                                (
                                                    "prefix".into(),
                                                    Value::from(prefix_policy.prefix()),
                                                ),
                                                (
                                                    "actions".into(),
                                                    Value::Array(
                                                        prefix_policy
                                                            .actions()
                                                            .iter()
                                                            .map(|action| {
                                                                Value::from(action.as_str())
                                                            })
                                                            .collect(),
                                                    ),
                                                ),
                                            ]))
                                        })
                                        .collect(),
                                ),
                            ),
                        ])),
                    )
                })
                .collect(),
        ),
        Err(problems) => Value::Object(Map::from_iter([(
            "errors".into(),
            Value::Array(
                problems
                    .iter()
                    .map(|problem| Value::from(problem.as_str()))
                    .collect(),
            ),
        )])),
    };
    resolved.insert("object_api_namespaces".into(), object_namespaces);
    // Which vault this machine's owner writes go through. Empty means the
    // machine discovers one, which is an answer only while it holds exactly
    // one; `stado host vaults` reads this key to say which of several a host
    // actually uses.
    resolved.insert(
        "skarbiec_vault_file".into(),
        Value::from(config::skarbiec_vault_file()),
    );
    resolved.insert(
        "release_skarbiec_url".into(),
        Value::from(config::release_skarbiec_url()),
    );
    resolved.insert(
        "release_skarbiec_consumer".into(),
        Value::from(config::release_skarbiec_consumer()),
    );
    resolved.insert(
        "release_skarbiec_token_file".into(),
        Value::from(config::release_skarbiec_token_file()),
    );
    let release_publishers = match config::release_api_publishers() {
        Ok(publishers) => Value::Object(
            publishers
                .iter()
                .map(|(product, policy)| {
                    (
                        product.clone(),
                        Value::Object(Map::from_iter([
                            ("item".into(), Value::from(policy.item())),
                            ("prefix".into(), Value::from(policy.prefix())),
                        ])),
                    )
                })
                .collect(),
        ),
        Err(problems) => Value::Object(Map::from_iter([(
            "errors".into(),
            Value::Array(
                problems
                    .iter()
                    .map(|problem| Value::from(problem.as_str()))
                    .collect(),
            ),
        )])),
    };
    resolved.insert("release_api_publishers".into(), release_publishers);
    resolved.insert(
        "service_skarbiec_url".into(),
        Value::from(config::service_skarbiec_url()),
    );
    resolved.insert(
        "service_skarbiec_consumer".into(),
        Value::from(config::service_skarbiec_consumer()),
    );
    resolved.insert(
        "service_skarbiec_token_file".into(),
        Value::from(config::service_skarbiec_token_file()),
    );
    let service_deployers = match config::service_api_deployers() {
        Ok(deployers) => Value::Object(
            deployers
                .iter()
                .map(|(product, policy)| {
                    (
                        product.clone(),
                        Value::Object(Map::from_iter([
                            ("consumer".into(), Value::from(policy.consumer())),
                            ("item".into(), Value::from(policy.item())),
                            (
                                "services".into(),
                                Value::Array(
                                    policy
                                        .services()
                                        .iter()
                                        .map(|service| Value::from(service.as_str()))
                                        .collect(),
                                ),
                            ),
                            (
                                "actions".into(),
                                Value::Array(
                                    policy
                                        .actions()
                                        .iter()
                                        .map(|action| Value::from(action.as_str()))
                                        .collect(),
                                ),
                            ),
                        ])),
                    )
                })
                .collect(),
        ),
        Err(problems) => Value::Object(Map::from_iter([(
            "errors".into(),
            Value::Array(
                problems
                    .iter()
                    .map(|problem| Value::from(problem.as_str()))
                    .collect(),
            ),
        )])),
    };
    resolved.insert("service_api_deployers".into(), service_deployers);
    resolved.insert(
        "agent_skarbiec_url".into(),
        Value::from(config::agent_skarbiec_url()),
    );
    resolved.insert(
        "agent_skarbiec_consumer".into(),
        Value::from(config::agent_skarbiec_consumer()),
    );
    resolved.insert(
        "agent_skarbiec_token_file".into(),
        Value::from(config::agent_skarbiec_token_file()),
    );
    resolved.insert(
        "agent_skarbiec_items".into(),
        Value::Array(
            config::agent_skarbiec_items()
                .iter()
                .map(|item| Value::from(item.as_str()))
                .collect(),
        ),
    );
    resolved.insert(
        "agent_skarbiec_secret_fields".into(),
        Value::Array(
            config::agent_skarbiec_secret_fields()
                .iter()
                .map(|reference| Value::from(reference.as_str()))
                .collect(),
        ),
    );
}
