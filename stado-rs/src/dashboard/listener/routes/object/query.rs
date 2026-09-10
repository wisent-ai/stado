//! The object coordinate one request addresses, and the metadata one write
//! publishes with it.

use std::collections::BTreeMap;

use serde_json::json;

use crate::remote::object_store::ObjectRef;

use crate::dashboard::listener::{http_status, parse_qs, query_value, send_json, Response};

pub(crate) fn object_from_query(query: &str) -> Result<crate::remote::object_store::ObjectRef, Response> {
    let values = parse_qs(query);
    let uri = query_value(&values, "uri").unwrap_or_default();
    if uri.is_empty() {
        return Err(send_json(
            http_status("400"),
            &json!({"error": "uri is required"}),
        ));
    }
    crate::remote::object_store::ObjectRef::parse(&uri)
        .map_err(|error| send_json(http_status("400"), &json!({"error": error.to_string()})))
}

pub(crate) fn public_release_object_from_query(
    query: &str,
) -> Result<crate::remote::object_store::ObjectRef, Response> {
    let values = parse_qs(query);
    if values.len() != 1 || values[0].0 != "uri" {
        return Err(send_json(
            http_status("400"),
            &json!({"error": "public release reads accept exactly one uri query field"}),
        ));
    }
    let uri = &values[0].1;
    if uri.is_empty() {
        return Err(send_json(
            http_status("400"),
            &json!({"error": "uri is required"}),
        ));
    }
    crate::remote::object_store::ObjectRef::parse(uri)
        .map_err(|error| send_json(http_status("400"), &json!({"error": error.to_string()})))
}

pub(crate) fn object_list_from_query(query: &str) -> Result<(String, String), Response> {
    let values = parse_qs(query);
    let raw_namespace = query_value(&values, "namespace").unwrap_or_default();
    if raw_namespace.is_empty() {
        return Err(send_json(
            http_status("400"),
            &json!({"error": "namespace is required"}),
        ));
    }
    let sentinel = crate::remote::object_store::ObjectRef::new(&raw_namespace, "sentinel")
        .map_err(|error| send_json(http_status("400"), &json!({"error": error.to_string()})))?;
    let namespace = sentinel.namespace().to_string();
    // Leading slashes are noise; a trailing one is the request. See
    // `ObjectRef::namespace_prefix`: trimming it turned `prefix=queue/` into a
    // scan of every `queue*` sibling.
    let prefix = query_value(&values, "prefix")
        .unwrap_or_default()
        .trim_start_matches('/')
        .to_string();
    crate::remote::object_store::ObjectRef::namespace_prefix(&namespace, &prefix)
        .map_err(|error| send_json(http_status("400"), &json!({"error": error.to_string()})))?;
    Ok((namespace, prefix))
}

pub(crate) fn merged_object_metadata(
    object: &ObjectRef,
    content_type: &str,
    extra: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, &'static str> {
    let mut metadata = crate::remote::object_store::metadata(object, content_type);
    for (name, value) in extra {
        if !name.starts_with("stado-")
            || metadata.contains_key(name)
            || value.is_empty()
            || name.chars().any(char::is_control)
            || value.chars().any(char::is_control)
        {
            return Err("custom object metadata must use unique non-empty stado-* fields");
        }
        metadata.insert(name.clone(), value.clone());
    }
    Ok(metadata)
}
