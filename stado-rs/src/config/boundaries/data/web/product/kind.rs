//! Which kind of declaration a web product entry is.

use crate::config::{canonical_machine_name, is_redirect_target};
use serde_json::{Map, Value};

/// Which kind one declaration is: a redirect, a hostname in front of an
/// existing service, or an ordinary unit product.
pub(super) fn parse_web_api_kind(
    name: &str,
    entry: &Map<String, Value>,
    problems: &mut Vec<String>,
) -> (Option<String>, Option<String>) {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct Fields {
        host: Option<serde::de::IgnoredAny>,
        port: Option<serde::de::IgnoredAny>,
        hostname: Option<serde::de::IgnoredAny>,
        consumer: Option<serde::de::IgnoredAny>,
        readyz: Option<serde::de::IgnoredAny>,
        edge: Option<serde::de::IgnoredAny>,
        env: Option<serde::de::IgnoredAny>,
        secrets: Option<serde::de::IgnoredAny>,
        database: Option<serde::de::IgnoredAny>,
        redirect_to: Option<serde::de::IgnoredAny>,
        upstream_service: Option<serde::de::IgnoredAny>,
        path_prefix: Option<serde::de::IgnoredAny>,
    }
    crate::config::boundaries::declared_fields::<Fields>(
        &Value::Object(entry.clone()),
        &format!("web_api.products.{name}"),
        problems,
    );
    // A redirect is a hostname and a target. Every other key describes a
    // unit — where it runs, as whom, on which port, with what environment
    // — and a declaration carrying both says two different things about
    // what this product is. Refusing the combination is how the reader of
    // this section never has to guess which half won.
    let redirect_to = match entry.get("redirect_to") {
        Some(Value::String(target)) if is_redirect_target(target) => Some(target.clone()),
        Some(other) => {
            problems.push(format!(
                "web_api.products.{name}.redirect_to {other} must be an https URL with a host and no query or fragment"
            ));
            None
        }
        None => None,
    };
    if redirect_to.is_some() {
        for key in [
            "host", "port", "consumer", "readyz", "env", "secrets", "database",
        ] {
            if entry.contains_key(key) {
                problems.push(format!(
                    "web_api.products.{name} declares redirect_to and {key}: a redirect has no unit, so it has no {key}"
                ));
            }
        }
    }
    // A hostname in front of an existing service names that service and
    // nothing about a unit: the service directory already says which host
    // it is active on and which address it answers, and repeating either
    // here would be a second copy that goes stale the day the service
    // moves.
    let upstream_service = match entry.get("upstream_service") {
        Some(Value::String(service)) if canonical_machine_name(service) => Some(service.clone()),
        Some(other) => {
            problems.push(format!(
                "web_api.products.{name}.upstream_service {other} must be a canonical registry service name"
            ));
            None
        }
        None => None,
    };
    if upstream_service.is_some() {
        for key in [
            "host", "port", "consumer", "readyz", "env", "secrets", "database",
        ] {
            if entry.contains_key(key) {
                problems.push(format!(
                    "web_api.products.{name} declares upstream_service and {key}: the service directory answers where that service runs, so this declaration has no {key}"
                ));
            }
        }
    }
    if upstream_service.is_some() && redirect_to.is_some() {
        problems.push(format!(
            "web_api.products.{name} declares both redirect_to and upstream_service: a hostname either answers with a redirect or forwards to a service, never both"
        ));
    }
    (redirect_to, upstream_service)
}
