//! The shapes a public-origin row's fields must have, each judged from the
//! string alone.

/// The longest name DNS carries written as text, as
/// <https://www.rfc-editor.org/rfc/rfc1035#section-2.3.4> bounds it (the wire
/// limit less the leading length octet and the root's terminating zero).
const MAX_NAME_TEXT: usize = 253;
/// The longest DNS label, as
/// <https://www.rfc-editor.org/rfc/rfc1035#section-2.3.4> bounds it.
const MAX_LABEL: usize = 63;

/// A lowercase identifier of letters, digits and single hyphens. No length is
/// imposed: the name is the argument `stado web origin` takes, and nothing
/// declares a bound on it.
pub(super) fn is_origin_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// A bare public DNS name: lowercase, at least two labels, no scheme, port,
/// path or userinfo, and not an address literal. An address literal is refused
/// because a certificate for a public origin is issued for a NAME.
pub(super) fn is_public_hostname(hostname: &str) -> bool {
    if hostname.len() > MAX_NAME_TEXT || hostname.ends_with('.') {
        return false;
    }
    if hostname.parse::<std::net::IpAddr>().is_ok() {
        return false;
    }
    // At least two labels: a name with a dot, and no empty label beside it.
    hostname.contains('.')
        && hostname.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= MAX_LABEL
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

/// A public origin's upstream is on the target's own loopback by
/// construction: the publication is what crosses the boundary, and an upstream
/// reachable from anywhere else would be a second, undeclared entrance. So it
/// is `http://`, names a loopback host (any loopback address or `localhost`,
/// as the shared origin check knows them) and an explicit non-zero port, and
/// carries no path.
pub(super) fn is_loopback_origin(upstream: &str) -> bool {
    let Some(authority) = upstream.strip_prefix("http://") else {
        return false;
    };
    if authority.contains('/') {
        return false;
    }
    let Some((_, port)) = authority.rsplit_once(':') else {
        return false;
    };
    port.parse::<std::num::NonZeroU16>().is_ok()
        && url::Url::parse(upstream)
            .is_ok_and(|url| crate::primitives::http::origin::is_loopback_host(&url))
}

/// An absolute path below the root: not `/` itself, no trailing slash, no
/// empty or `..` segment, no query or fragment, ASCII only.
pub(super) fn is_published_path(path: &str) -> bool {
    path.starts_with('/')
        && path != "/"
        && !path.ends_with('/')
        && !path.contains("//")
        && !path.split('/').any(|segment| segment == "..")
        && !path.contains('?')
        && !path.contains('#')
        && path.is_ascii()
}
