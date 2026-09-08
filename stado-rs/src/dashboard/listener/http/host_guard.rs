//! The Host-header guard every authenticated route answers behind, and the
//! DNS label a host beacon may publish under.

use crate::config;
use crate::dashboard::listener::Dashboard;

// ---------------------------------------------------------------------------
// Host-header DNS-rebinding guard
// ---------------------------------------------------------------------------

/// Accept loopback Host values for direct local access. A configured HTTPS
/// reverse proxy may forward either DNS or IP Host values; because the listener
/// itself is loopback-only, `X-Forwarded-Proto` cannot be supplied by external
/// plaintext ingress. Dashboard authorization remains a separate boundary.
pub(crate) fn trusted_request_host(
    value: Option<&str>,
    forwarded_proto: Option<&str>,
    reverse_proxy_enabled: bool,
) -> bool {
    let Some(value) = value else { return false };
    if value.is_empty() {
        return false;
    }
    // Malformed authorities always fail closed.
    let proxy_https = reverse_proxy_enabled && forwarded_proto == Some("https");

    // authority = [userinfo@]host[:port]; path/query/fragment split off.
    let (authority, has_suffix) = match value.find(['/', '?', '#']) {
        Some(index) => (&value[..index], true),
        None => (value, false),
    };
    let (userinfo, host_port) = match authority.rsplit_once('@') {
        Some((userinfo, host_port)) => (Some(userinfo), host_port),
        None => (None, authority),
    };
    let (host, port) = if let Some(rest) = host_port.strip_prefix('[') {
        match rest.split_once(']') {
            Some((host, after)) => {
                let port = if after.is_empty() {
                    None
                } else if let Some(port) = after.strip_prefix(':') {
                    Some(port)
                } else {
                    // "[::1]junk" — Python raises ValueError on .hostname.
                    return false;
                };
                (host, port)
            }
            // Unterminated bracket — urlsplit raises ValueError.
            None => return false,
        }
    } else {
        match host_port.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (host_port, None),
        }
    };
    // Python `_ = parsed.port`: an unparseable/out-of-range port raises
    // ValueError -> the DNS branch (even for IP hosts).
    if let Some(port) = port {
        if port.parse::<u16>().is_err() {
            return false;
        }
    }
    // Python: `if not host or parsed.username or parsed.password or
    // parsed.path or parsed.query or parsed.fragment: return False`.
    let (username, password) = match userinfo {
        Some(userinfo) => match userinfo.split_once(':') {
            Some((name, password)) => (name, Some(password)),
            None => (userinfo, None),
        },
        None => ("", None),
    };
    if host.is_empty()
        || !username.is_empty()
        || password.is_some_and(|password| !password.is_empty())
        || has_suffix
    {
        return false;
    }
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(address) => address.is_loopback() || proxy_https,
        Err(_) => proxy_https,
    }
}

pub(crate) fn valid_beacon_host(host: &str) -> bool {
    let bytes = host.as_bytes();
    !bytes.is_empty()
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

impl Dashboard {
    fn deployment_id(&self) -> String {
        config::stado_deployment_id()
    }

    pub(crate) fn trusted_request_host(
        &self,
        value: Option<&str>,
        forwarded_proto: Option<&str>,
    ) -> bool {
        let reverse_proxy_enabled =
            config::dashboard_trust_https_proxy() || !self.deployment_id().is_empty();
        trusted_request_host(value, forwarded_proto, reverse_proxy_enabled)
    }
}
