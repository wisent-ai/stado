//! The object coordinate one request addresses, and the metadata one write
//! publishes with it.

use std::collections::BTreeMap;

use serde_json::json;

use crate::remote::object_store::ObjectRef;

use crate::dashboard::listener::{http_status, parse_qs, query_value, send_json, Response};

pub(crate) fn object_from_query(
    query: &str,
) -> Result<crate::remote::object_store::ObjectRef, Response> {
    let values = parse_qs(query)?;
    let uri = match query_value(&values, "uri") {
        Some(uri) if !uri.is_empty() => uri,
        _ => {
            return Err(send_json(
                http_status(reqwest::StatusCode::BAD_REQUEST),
                &json!({"error": "uri is required"}),
            ))
        }
    };
    crate::remote::object_store::ObjectRef::parse(&uri).map_err(|error| {
        send_json(
            http_status(reqwest::StatusCode::BAD_REQUEST),
            &json!({"error": error.to_string()}),
        )
    })
}

pub(crate) fn public_release_object_from_query(
    query: &str,
) -> Result<crate::remote::object_store::ObjectRef, Response> {
    let values = parse_qs(query)?;
    let uri = match values.as_slice() {
        [(key, uri)] if key == "uri" => uri,
        _ => {
            return Err(send_json(
                http_status(reqwest::StatusCode::BAD_REQUEST),
                &json!({"error": "public release reads accept exactly one uri query field"}),
            ))
        }
    };
    if uri.is_empty() {
        return Err(send_json(
            http_status(reqwest::StatusCode::BAD_REQUEST),
            &json!({"error": "uri is required"}),
        ));
    }
    crate::remote::object_store::ObjectRef::parse(uri).map_err(|error| {
        send_json(
            http_status(reqwest::StatusCode::BAD_REQUEST),
            &json!({"error": error.to_string()}),
        )
    })
}

pub(crate) fn object_list_from_query(query: &str) -> Result<(String, String), Response> {
    let values = parse_qs(query)?;
    let raw_namespace = match query_value(&values, "namespace") {
        Some(namespace) if !namespace.is_empty() => namespace,
        _ => {
            return Err(send_json(
                http_status(reqwest::StatusCode::BAD_REQUEST),
                &json!({"error": "namespace is required"}),
            ))
        }
    };
    let sentinel = crate::remote::object_store::ObjectRef::new(&raw_namespace, "sentinel")
        .map_err(|error| {
            send_json(
                http_status(reqwest::StatusCode::BAD_REQUEST),
                &json!({"error": error.to_string()}),
            )
        })?;
    let namespace = sentinel.namespace().to_string();
    // Leading slashes are noise; a trailing one is the request. See
    // `ObjectRef::namespace_prefix`: trimming it turned `prefix=queue/` into a
    // scan of every `queue*` sibling. A listing without a prefix lists the
    // whole namespace.
    let prefix = match query_value(&values, "prefix") {
        Some(prefix) => prefix.trim_start_matches('/').to_string(),
        None => String::new(),
    };
    crate::remote::object_store::ObjectRef::namespace_prefix(&namespace, &prefix).map_err(
        |error| {
            send_json(
                http_status(reqwest::StatusCode::BAD_REQUEST),
                &json!({"error": error.to_string()}),
            )
        },
    )?;
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
