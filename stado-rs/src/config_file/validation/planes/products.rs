//! The planes a product declares for itself: its object namespaces, its
//! databases, its web products and edge, and its release publishers. They are
//! verified through Stado's Skarbiec identity.

use serde_json::{Map, Value};

use crate::config_file::readers::field_in;

/// The product-object plane: namespaces that parse and cover what the queue
/// reads.
pub(in crate::config_file::validation) fn object_api(
    root: &Map<String, Value>,
    problems: &mut Vec<String>,
) {
    let object_api = root.get("object_api").and_then(Value::as_object);
    if object_api.is_some() {
        match crate::config::parse_object_api_namespaces(field_in(
            root,
            &crate::capabilities::OBJECT_API_NAMESPACES_CONFIG,
        )) {
            Err(object_problems) => problems.extend(object_problems),
            // The policy parses; now it has to cover what the queue reads and
            // writes, or `config set` would write a document under which the
            // agent's next claim answers 401.
            Ok(namespaces) => problems.extend(crate::config::queue_prefix_problem(&namespaces)),
        }
    }
}

/// The declared databases, judged by the parser that reads them.
pub(in crate::config_file::validation) fn database_api(
    root: &Map<String, Value>,
    problems: &mut Vec<String>,
) {
    let database_api = root.get("database_api").and_then(Value::as_object);
    if database_api.is_some() {
        if let Err(database_problems) = crate::config::parse_database_api_databases(field_in(
            root,
            &crate::capabilities::DATABASE_API_DATABASES_CONFIG,
        )) {
            problems.extend(database_problems);
        }
    }
}

/// The web plane's two independent halves, and the refusal of any third key.
pub(in crate::config_file::validation) fn web_api(
    root: &Map<String, Value>,
    problems: &mut Vec<String>,
) {
    // Both halves of the web plane are optional and independent: a fleet can
    // declare products before its edge exists, and an edge is worth declaring
    // before the first product moves onto it.
    if let Some(web_api) = root.get("web_api").and_then(Value::as_object) {
        for key in web_api.keys() {
            if !matches!(key.as_str(), "products" | "edge") {
                problems.push(format!("web_api contains unsupported key {key:?}"));
            }
        }
        if web_api.contains_key("products") {
            if let Err(web_problems) = crate::config::parse_web_api_products(field_in(
                root,
                &crate::capabilities::WEB_API_PRODUCTS_CONFIG,
            )) {
                problems.extend(web_problems);
            }
        }
        if web_api.contains_key("edge") {
            if let Err(edge_problems) = crate::config::parse_web_api_edge(web_api.get("edge")) {
                problems.extend(edge_problems);
            }
        }
    }
}

/// The immutable-release plane: publishers that parse.
pub(in crate::config_file::validation) fn release_api(
    root: &Map<String, Value>,
    problems: &mut Vec<String>,
) {
    let release_api = root.get("release_api").and_then(Value::as_object);
    if release_api.is_some() {
        if let Err(release_problems) = crate::config::parse_release_publishers(field_in(
            root,
            &crate::capabilities::RELEASE_API_PUBLISHERS_CONFIG,
        )) {
            problems.extend(release_problems);
        }
    }
}
