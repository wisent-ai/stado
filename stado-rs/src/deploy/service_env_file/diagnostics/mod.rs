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

#[cfg(test)]
mod ordering_and_verdicts;

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
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 2 {
        let head = bytes[usize::MIN];
        if (head == b'"' || head == b'\'') && bytes[bytes.len() - 1] == head {
            return &trimmed[1..trimmed.len() - 1];
        }
    }
    // `KEY=value # note` is a comment to every shell that sources the file,
    // and only for an unquoted value.
    match trimmed.split_once(" #") {
        Some((head, _)) => head.trim_end(),
        None => trimmed,
    }
}

/// Whether an authority names this machine.
fn authority_is_loopback(authority: &str) -> bool {
    matches!(
        authority,
        "127.0.0.1" | "localhost" | "::1" | "[::1]" | "0.0.0.0" | "*"
    )
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
    if let Some((scheme, rest)) = text.split_once("://") {
        if scheme.is_empty()
            || !scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '.' || c == '-')
        {
            return None;
        }
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        // A URL carrying userinfo is never reported by the host, so reaching
        // this with one means an operator revealed it; the credential half is
        // not part of the endpoint either way.
        let authority = authority.rsplit('@').next().unwrap_or_default();
        let (host, port) = match authority.strip_prefix('[') {
            // `[::1]:8895` — the colon that separates the port is the one
            // after the closing bracket, not the ones inside it.
            Some(rest) => match rest.split_once("]:") {
                Some((host, port)) => (format!("[{host}]"), Some(port)),
                None => (authority.to_string(), None),
            },
            None => match authority.rsplit_once(':') {
                Some((host, port)) => (host.to_string(), Some(port)),
                None => (authority.to_string(), None),
            },
        };
        let port = match port {
            Some(port) => port.parse::<u32>().ok()?,
            None => match scheme {
                "http" | "ws" => 80,
                "https" | "wss" => 443,
                _ => return None,
            },
        };
        if port == u32::MIN || port > u32::from(u16::MAX) {
            return None;
        }
        return Some(Endpoint {
            port,
            loopback: authority_is_loopback(&host),
        });
    }
    if key == "PORT" || key.ends_with("_PORT") {
        let port = text.parse::<u32>().ok()?;
        if port == u32::MIN || port > u32::from(u16::MAX) {
            return None;
        }
        return Some(Endpoint {
            port,
            loopback: true,
        });
    }
    // `127.0.0.1:8895` with no scheme, the spelling a `*_HOST` or `*_ADDR`
    // variable usually carries.
    let (host, port) = text.rsplit_once(':')?;
    let port = port.parse::<u32>().ok()?;
    if port == u32::MIN
        || port > u32::from(u16::MAX)
        || host.is_empty()
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
