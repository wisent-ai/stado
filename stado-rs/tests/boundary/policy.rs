//! What this listener is configured to authorize: every active object
//! namespace with its own item, and every active release publisher with its
//! own.
//!
//! Both documents are built from the product's own constants
//! (`config::ACTIVE_OBJECT_NAMESPACES`, `config::ACTIVE_RELEASE_PUBLISHERS`,
//! `queue::copy::CANONICAL_PREFIXES`) rather than from a copied list, because
//! a verifier refuses a policy that leaves an active declaration out and a
//! hand-kept list would decide for itself what the fleet declares. Nothing
//! here is a value that tunes the product: the prefixes and actions are the
//! ones the queue's own reader requires of the namespace it lives in.

#![allow(dead_code)]

use serde_json::{json, Map, Value};

use crate::vault::{object_item, publisher_item};

/// The namespace the object cases read through, and the key they address: the
/// queue's own namespace, and a queue object this store does not hold.
pub const NAMESPACE: &str = stado::config::QUEUE_OBJECT_NAMESPACE;
pub const ABSENT_KEY: &str = "queue/boundary-area-absent.json";

/// A release coordinate no declared publisher covers. Its namespace makes it a
/// release-governed key — which is what puts the release boundary in the
/// request's plan — while no `release_api.publishers` entry claims the
/// product, so the request is refused for its key and never for a bearer.
pub const UNDECLARED_RELEASE_KEY: &str = "boundary-area-probe/1.0.0/stado";

/// Every object item this listener's policy names.
pub fn object_items() -> Vec<String> {
    stado::config::ACTIVE_OBJECT_NAMESPACES
        .iter()
        .map(|namespace| object_item(namespace))
        .collect()
}

/// Every release-publisher item this listener's policy names.
pub fn publisher_items() -> Vec<String> {
    stado::config::ACTIVE_RELEASE_PUBLISHERS
        .iter()
        .map(|product| publisher_item(product))
        .collect()
}

/// The `WC_OBJECT_API_NAMESPACES` document.
///
/// The queue's namespace grants the queue's own canonical prefixes for every
/// object action, because a policy that leaves one ungranted is a deployment
/// defect the verifier reports instead of a boundary it can open.
pub fn namespaces() -> String {
    let mut document = Map::new();
    for namespace in stado::config::ACTIVE_OBJECT_NAMESPACES {
        let prefixes: Vec<&str> = if *namespace == NAMESPACE {
            stado::queue::copy::CANONICAL_PREFIXES.to_vec()
        } else {
            vec!["data/"]
        };
        document.insert(
            (*namespace).to_string(),
            json!({
                "item": object_item(namespace),
                "prefixes": prefixes,
                "actions": stado::config::OBJECT_API_ACTIONS,
            }),
        );
    }
    Value::Object(document).to_string()
}

/// The `WC_RELEASE_API_PUBLISHERS` document: every active publisher, each with
/// the item and prefix the product requires it to declare.
pub fn publishers() -> String {
    let mut document = Map::new();
    for product in stado::config::ACTIVE_RELEASE_PUBLISHERS {
        document.insert(
            (*product).to_string(),
            json!({
                "item": publisher_item(product),
                "prefix": format!("{product}/"),
            }),
        );
    }
    Value::Object(document).to_string()
}
