//! Host, port, hostname and mount prefix of one web product.

use crate::config::{canonical_machine_name, is_mount_prefix, is_public_hostname};
use serde_json::{Map, Value};

/// Where one product runs and which name it answers on.
pub(super) fn parse_web_api_placement(
    name: &str,
    entry: &Map<String, Value>,
    redirect_to: Option<&str>,
    upstream_service: Option<&str>,
    problems: &mut Vec<String>,
) -> (String, u16, String, Option<String>) {
    let host = match entry.get("host").and_then(Value::as_str) {
        Some(host) if canonical_machine_name(host) => host.to_string(),
        Some(other) => {
            problems.push(format!(
                "web_api.products.{name}.host {other:?} is not a canonical target name"
            ));
            String::new()
        }
        // A redirect runs nowhere, so it names no host. The keys are
        // refused above when both are present, so this arm only ever
        // sees a declaration that is a redirect and says so.
        None if redirect_to.is_some() || upstream_service.is_some() => String::new(),
        None => {
            problems.push(format!("web_api.products.{name}.host is required"));
            String::new()
        }
    };
    // Anything below 1024 needs privilege these units deliberately do not
    // have: a web unit runs as the same login account every other managed
    // unit runs as, and the edge is what owns 443.
    let port = match entry.get("port").and_then(Value::as_u64) {
        Some(port) if (1024..=65535).contains(&port) => port as u16,
        Some(other) => {
            problems.push(format!(
                "web_api.products.{name}.port {other} must be between 1024 and 65535"
            ));
            0
        }
        None if redirect_to.is_some() || upstream_service.is_some() => 0,
        None => {
            problems.push(format!("web_api.products.{name}.port is required"));
            0
        }
    };
    let hostname = match entry.get("hostname").and_then(Value::as_str) {
        Some(hostname) if is_public_hostname(hostname) => hostname.to_string(),
        Some(other) => {
            problems.push(format!(
                "web_api.products.{name}.hostname {other:?} is not a public host name"
            ));
            String::new()
        }
        None => {
            problems.push(format!("web_api.products.{name}.hostname is required"));
            String::new()
        }
    };
    // A path prefix a product is mounted at, under a hostname another
    // declaration owns. Written before the hostname bookkeeping below
    // because that bookkeeping now depends on it: one hostname has
    // exactly one owner and any number of mounts, and each mount holds a
    // distinct prefix.
    let path_prefix = match entry.get("path_prefix") {
        Some(Value::String(prefix)) if is_mount_prefix(prefix) => Some(prefix.clone()),
        Some(other) => {
            problems.push(format!(
                "web_api.products.{name}.path_prefix {other} must be an absolute path with no trailing slash, like \"/docs\""
            ));
            None
        }
        None => None,
    };
    // A mount is an ordinary unit product; what it must not be is one of
    // the two hostname-only kinds. A redirect answers the whole hostname
    // and an upstream service forwards the whole hostname, so neither can
    // also be a path under someone else's.
    if path_prefix.is_some() {
        for (key, why) in [
            ("redirect_to", "a redirect answers a whole hostname"),
            (
                "upstream_service",
                "a hostname in front of a service forwards the whole hostname",
            ),
        ] {
            if entry.contains_key(key) {
                problems.push(format!(
                    "web_api.products.{name} declares path_prefix and {key}: {why}, so it cannot also be a path under another product's hostname"
                ));
            }
        }
    }
    (host, port, hostname, path_prefix)
}
