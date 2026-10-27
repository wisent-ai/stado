//! The declared edge host that terminates TLS for Stado-served hostnames.

use std::sync::LazyLock;

use crate::config::canonical_machine_name;
use serde_json::Value;

/// The declared edge host: the fleet target that holds a public address and
/// terminates TLS for every `edge: "stado"` hostname.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebApiEdge {
    target: String,
    address: String,
    contact: String,
    registrar_credential: Option<String>,
}

impl WebApiEdge {
    pub fn target(&self) -> &str {
        &self.target
    }

    /// The public IPv4 address the product hostnames' A records point at.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// The address Let's Encrypt sends expiry mail to.
    pub fn contact(&self) -> &str {
        &self.contact
    }

    /// The Skarbiec item holding the registrar's `api_user`, `api_key`,
    /// `username` and `client_ip`, through which the A records are written.
    pub fn registrar_credential(&self) -> Option<&str> {
        self.registrar_credential.as_deref()
    }
}

pub(crate) fn parse_web_api_edge(value: Option<&Value>) -> Result<WebApiEdge, Vec<String>> {
    let Some(object @ Value::Object(entry)) = value else {
        return Err(vec![
            "web_api.edge must be an object with target, address and contact".to_string(),
        ]);
    };
    let mut problems = Vec::new();
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct Fields {
        target: Option<serde::de::IgnoredAny>,
        address: Option<serde::de::IgnoredAny>,
        contact: Option<serde::de::IgnoredAny>,
        registrar_credential: Option<serde::de::IgnoredAny>,
    }
    crate::config::boundaries::declared_fields::<Fields>(
        object,
        "web_api.edge",
        &mut problems,
    );
    // Each required field is read as itself: absent, not a string, and a
    // string of the wrong shape are three problems with three sentences.
    let required = |name: &str, problems: &mut Vec<String>| -> Option<String> {
        match entry.get(name) {
            Some(Value::String(text)) => Some(text.clone()),
            Some(other) => {
                problems.push(format!("web_api.edge.{name} must be a string, not {other}"));
                None
            }
            None => {
                problems.push(format!("web_api.edge.{name} is required"));
                None
            }
        }
    };
    let mut target = required("target", &mut problems);
    if target
        .as_deref()
        .is_some_and(|target| !canonical_machine_name(target))
    {
        problems.push("web_api.edge.target must be a canonical target".to_string());
        target = None;
    }
    let mut address = required("address", &mut problems);
    if let Some(Err(error)) = address.as_deref().map(str::parse::<std::net::Ipv4Addr>) {
        problems.push(format!(
            "web_api.edge.address must be an IPv4 address: {error}"
        ));
        address = None;
    }
    let mut contact = required("contact", &mut problems);
    if contact
        .as_deref()
        .is_some_and(|contact| !contact.contains('@') || contact.chars().any(char::is_whitespace))
    {
        problems.push("web_api.edge.contact must be a mail address".to_string());
        contact = None;
    }
    let registrar_credential = match entry.get("registrar_credential") {
        None => None,
        Some(Value::String(item)) if !item.trim().is_empty() => Some(item.clone()),
        Some(_) => {
            problems
                .push("web_api.edge.registrar_credential must name a Skarbiec item".to_string());
            None
        }
    };
    match (target, address, contact, problems.is_empty()) {
        (Some(target), Some(address), Some(contact), true) => Ok(WebApiEdge {
            target,
            address,
            contact,
            registrar_credential,
        }),
        _ => Err(problems),
    }
}

static WEB_API_EDGE: LazyLock<Result<WebApiEdge, Vec<String>>> =
    LazyLock::new(|| parse_web_api_edge(crate::config_file::get("web_api.edge").as_ref()));

pub fn web_api_edge() -> Result<&'static WebApiEdge, &'static [String]> {
    match &*WEB_API_EDGE {
        Ok(edge) => Ok(edge),
        Err(problems) => Err(problems.as_slice()),
    }
}
