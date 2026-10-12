//! Which transport a URL names, and whether its host is this machine.
//!
//! The two web schemes (RFC 9110 section 4.2) are a type, parsed once from
//! the URL's scheme. Every check that an address is a web URL, that it is
//! encrypted, or that plain HTTP is allowed because the host is local asks
//! this module instead of comparing scheme strings at each call site.

/// A web URL's scheme; any other scheme is not a web URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WebScheme {
    Http,
    Https,
}

impl WebScheme {
    /// The URL's web scheme, or `None` for any other scheme.
    pub fn of(url: &url::Url) -> Option<Self> {
        <Self as serde::Deserialize>::deserialize(serde_json::Value::String(
            url.scheme().to_string(),
        ))
        .ok()
    }
}

/// The URL is HTTPS.
pub fn is_secure(url: &url::Url) -> bool {
    WebScheme::of(url) == Some(WebScheme::Https)
}

/// The URL is HTTP or HTTPS.
pub fn is_web(url: &url::Url) -> bool {
    WebScheme::of(url).is_some()
}

/// HTTPS, or plain HTTP to a host the caller has established is local
/// (`local`), the one case where clear text does not leave the machine or an
/// already encrypted private network.
pub fn is_secure_or_local_plain(url: &url::Url, local: bool) -> bool {
    match WebScheme::of(url) {
        Some(WebScheme::Https) => true,
        Some(WebScheme::Http) => local,
        None => false,
    }
}

/// The URL's host is this machine: the `localhost` name or a loopback IP.
pub fn is_loopback_host(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}
