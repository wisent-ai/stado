//! Shared test data only, and no assertions of its own: the declared edge the
//! rendering checks render against, and the site blocks they build from.

use super::*;

use serde_json::json;

/// The site blocks for a set of one-directive hostnames, which is what
/// every product except a mount produces.
pub(super) fn blocks(routes: Vec<(String, String)>) -> Vec<(String, Vec<String>)> {
    routes
        .into_iter()
        .map(|(hostname, directive)| (hostname, vec![directive]))
        .collect()
}

pub(super) fn edge() -> WebApiEdge {
    config::parse_web_api_edge(Some(&json!({
        "target": "wisent-edge",
        "address": "20.12.34.56",
        "contact": "operator@wisent.com",
    })))
    .expect("a complete edge declaration parses")
}
