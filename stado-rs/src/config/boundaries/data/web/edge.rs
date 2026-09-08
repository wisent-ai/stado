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
}

pub(crate) fn parse_web_api_edge(value: Option<&Value>) -> Result<WebApiEdge, Vec<String>> {
    let Some(Value::Object(entry)) = value else {
        return Err(vec![
            "web_api.edge must be an object with target, address and contact".to_string(),
        ]);
    };
    let mut problems = Vec::new();
    for key in entry.keys() {
        if !matches!(key.as_str(), "target" | "address" | "contact") {
            problems.push(format!("web_api.edge contains unsupported key {key:?}"));
        }
    }
    let target = entry.get("target").and_then(Value::as_str).unwrap_or("");
    if !canonical_machine_name(target) {
        problems.push("web_api.edge.target is required and must be a canonical target".to_string());
    }
    let address = entry.get("address").and_then(Value::as_str).unwrap_or("");
    if address.parse::<std::net::Ipv4Addr>().is_err() {
        problems.push("web_api.edge.address is required and must be an IPv4 address".to_string());
    }
    let contact = entry.get("contact").and_then(Value::as_str).unwrap_or("");
    if !contact.contains('@') || contact.chars().any(char::is_whitespace) {
        problems.push("web_api.edge.contact is required and must be a mail address".to_string());
    }
    if problems.is_empty() {
        Ok(WebApiEdge {
            target: target.to_string(),
            address: address.to_string(),
            contact: contact.to_string(),
        })
    } else {
        Err(problems)
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
