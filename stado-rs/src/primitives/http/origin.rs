//! Which transport a URL names, and whether its host is this machine.
//!
//! The two web schemes (RFC 9110 section 4.2) are named once here. Every
//! check that an address is a web URL, that it is encrypted, or that plain
//! HTTP is allowed because the host is local asks this module instead of
//! comparing scheme strings and loopback spellings at each call site.

const SECURE: &str = "https";
const PLAIN: &str = "http";

/// The URL is HTTPS.
pub fn is_secure(url: &url::Url) -> bool {
    url.scheme() == SECURE
}

/// The URL is HTTP or HTTPS.
pub fn is_web(url: &url::Url) -> bool {
    is_secure(url) || url.scheme() == PLAIN
}

/// HTTPS, or plain HTTP to a host the caller has established is local
/// (`local`), the one case where clear text does not leave the machine or an
/// already encrypted private network.
pub fn is_secure_or_local_plain(url: &url::Url, local: bool) -> bool {
    is_secure(url) || (local && url.scheme() == PLAIN)
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
