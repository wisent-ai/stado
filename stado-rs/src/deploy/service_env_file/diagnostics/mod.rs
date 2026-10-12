//! The diagnostics: what a value MEANS, which assignment of a key a sourced
//! file actually leaves behind, and whether anything answers where an
//! assignment says it should.
//!
//! Everything here reads the report the host already decided to send. It runs
//! on this side, not on the host, so it can be exercised against the shapes
//! real env files use.

use super::*;

mod precedence;
mod reconcile;

pub use precedence::{duplicate_keys, shadowing};
pub use reconcile::{endpoint_rows, endpoint_verdict, EndpointRow};

/// The value a shell would end up with, quotes removed and a trailing
/// unquoted comment dropped.
///
/// Interpretation lives here rather than on the host so it can be tested
/// against the shapes real env files use. The host's copy of the unquoting
/// exists for one reason only — to decide what may be shown — and this one
/// decides what the value MEANS.
pub fn effective_text(value: &str) -> &str {
    let trimmed = value.trim();
    for quote in ['"', '\''] {
        if let Some(inner) = trimmed
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner;
        }
    }
    // `KEY=value # note` is a comment to every shell that sources the file,
    // and only for an unquoted value.
    match trimmed.split_once(" #") {
        Some((head, _)) => head.trim_end(),
        None => trimmed,
    }
}

/// Whether an authority names this machine: the `localhost` name, a loopback
/// or unspecified (bind-all) IP, or the `*` bind-all wildcard listeners print.
fn authority_is_loopback(authority: &str) -> bool {
    if authority == "*" {
        return true;
    }
    match url::Host::parse(authority) {
        Ok(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        Ok(url::Host::Ipv4(address)) => address.is_loopback() || address.is_unspecified(),
        Ok(url::Host::Ipv6(address)) => address.is_loopback() || address.is_unspecified(),
        Err(_) => false,
    }
}

/// The endpoint one assignment declares, or `None` for a value that is not an
/// endpoint at all.
///
/// A bare integer is only read as a port for a key that says it is one.
/// `WELES_MAX_CONCURRENCY=4` is not a declaration that something must be
/// listening on port 4, and reporting it as a dead dependency would make this
/// command's non-zero exit meaningless.
pub fn declared_endpoint(key: &str, value: &str) -> Option<Endpoint> {
    let text = effective_text(value);
    if text.is_empty() {
        return None;
    }
    if text.contains("://") {
        // The URL parser knows each special scheme's default port (http, ws,
        // https, wss, ftp); a scheme with none and no explicit port is not an
        // endpoint this command can probe.
        let url = url::Url::parse(text).ok()?;
        let port = std::num::NonZeroU16::new(url.port_or_known_default()?)?;
        return Some(Endpoint {
            port: u32::from(port.get()),
            loopback: authority_is_loopback(url.host_str()?),
        });
    }
    if key == "PORT" || key.ends_with("_PORT") {
        let port = tcp_port(text)?;
        return Some(Endpoint {
            port,
            loopback: true,
        });
    }
    // `127.0.0.1:8895` with no scheme, the spelling a `*_HOST` or `*_ADDR`
    // variable usually carries.
    let (host, port) = text.rsplit_once(':')?;
    let port = tcp_port(port)?;
    if host.is_empty()
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        return None;
    }
    Some(Endpoint {
        port,
        loopback: authority_is_loopback(host),
    })
}

/// A TCP port: a number a socket can carry, and never zero.
fn tcp_port(text: &str) -> Option<u32> {
    text.parse::<std::num::NonZeroU16>()
        .ok()
        .map(|port| u32::from(port.get()))
}
