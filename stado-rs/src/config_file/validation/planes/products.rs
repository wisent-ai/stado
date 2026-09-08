//! The planes a product declares for itself: its object namespaces, its
//! databases, its web products and edge, and its release publishers. Each
//! verifier grant has to be distinct from every grant declared before it.

use serde_json::{Map, Value};

use crate::config_file::readers::{field_in, py_truthy};

/// The product-object plane: namespaces that parse and cover what the queue
/// reads, and a verifier grant that is not the coordinator's.
pub(in crate::config_file::validation) fn object_api(
    root: &Map<String, Value>,
    control_token_file: &str,
    object_token_file: &str,
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
        let object_skarbiec = object_api
            .and_then(|section| section.get("skarbiec"))
            .and_then(Value::as_object);
        if field_in(root, &crate::capabilities::OBJECT_API_SKARBIEC.url)
            .is_some_and(|url| !py_truthy(url))
        {
            problems.push(
                "object_api.skarbiec.url, when set, must be a non-empty verifier endpoint"
                    .to_string(),
            );
        }
        if field_in(root, &crate::capabilities::OBJECT_API_SKARBIEC.consumer)
            .and_then(Value::as_str)
            != Some(crate::config::OBJECT_API_VERIFIER_CONSUMER)
        {
            problems.push(format!(
                "object_api.skarbiec.consumer must be the dedicated least-privilege consumer {:?}",
                crate::config::OBJECT_API_VERIFIER_CONSUMER
            ));
        }
        if object_token_file.is_empty() {
            problems.push(
                "object_api.skarbiec.token_file must name the owner-only verifier grant file"
                    .to_string(),
            );
        }
        if !object_token_file.is_empty() && object_token_file == control_token_file {
            problems.push(
            "object_api.skarbiec.token_file must be distinct from the coordinator Skarbiec grant"
                .to_string(),
        );
        }
        if object_skarbiec.is_some_and(|section| section.contains_key("token")) {
            problems.push(
            "object_api.skarbiec.token is forbidden; store the verifier grant only in its owner-only token_file"
                .to_string(),
        );
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

/// The immutable-release plane: publishers that parse, and a verifier grant
/// distinct from the coordinator's and the product-object one.
pub(in crate::config_file::validation) fn release_api(
    root: &Map<String, Value>,
    control_token_file: &str,
    object_token_file: &str,
    release_token_file: &str,
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
        let release_skarbiec = release_api
            .and_then(|section| section.get("skarbiec"))
            .and_then(Value::as_object);
        if field_in(root, &crate::capabilities::RELEASE_API_SKARBIEC.url)
            .is_some_and(|url| !py_truthy(url))
        {
            problems.push(
                "release_api.skarbiec.url, when set, must be a non-empty verifier endpoint"
                    .to_string(),
            );
        }
        if field_in(root, &crate::capabilities::RELEASE_API_SKARBIEC.consumer)
            .and_then(Value::as_str)
            != Some(crate::config::RELEASE_API_VERIFIER_CONSUMER)
        {
            problems.push(format!(
                "release_api.skarbiec.consumer must be the dedicated least-privilege consumer {:?}",
                crate::config::RELEASE_API_VERIFIER_CONSUMER
            ));
        }
        if release_token_file.is_empty() {
            problems.push(
            "release_api.skarbiec.token_file must name the owner-only release verifier grant file"
                .to_string(),
        );
        }
        if !release_token_file.is_empty()
            && (release_token_file == control_token_file || release_token_file == object_token_file)
        {
            problems.push(
            "release_api.skarbiec.token_file must be distinct from coordinator and product-object verifier grants"
                .to_string(),
        );
        }
        if release_skarbiec.is_some_and(|section| section.contains_key("token")) {
            problems.push(
            "release_api.skarbiec.token is forbidden; store the verifier grant only in its owner-only token_file"
                .to_string(),
        );
        }
    }
}
