//! One declaration, one directive: a mount's `handle_path`, a redirect's
//! `redir`, and a route's `reverse_proxy`.

use super::CmdError;
use crate::config;

/// One mount: the hostname it lives under, and the `handle_path` block that
/// answers its prefix from its own unit.
///
/// `handle_path` rather than `handle`, because it strips the matched prefix
/// before proxying: the unit behind `/docs` is a site whose own paths start
/// at `/`, so `/docs/core` has to arrive as `/core`. `handle` would forward
/// `/docs/core` unchanged and every page would answer 404.
///
/// The matcher is `<prefix>*` so that the prefix itself matches as well as
/// everything under it — `/docs` and `/docs/core` are both this mount's.
pub(in crate::cli::web) fn mount(
    hostname: &str,
    prefix: &str,
    host: &str,
    port: u16,
) -> Result<(String, String), CmdError> {
    if !config::is_mount_prefix(prefix) {
        return Err(CmdError::click(format!(
            "{prefix:?} is not a mount prefix: an absolute path with no trailing slash, like \"/docs\""
        )));
    }
    let (hostname, upstream) = route(hostname, host, port)?;
    Ok((
        hostname,
        format!("handle_path {prefix}* {{\n\t\t{upstream}\n\t}}"),
    ))
}

/// One redirect: the public hostname the edge terminates, and the `redir`
/// directive that answers every request on it.
///
/// 308 rather than 301: it preserves the method and the body, so a `POST` to
/// the old hostname arrives at the new one as a `POST`. 301 lets a client turn
/// it into a `GET`, which is how a form submission silently becomes a page
/// load. Permanent either way, because these hostnames are not coming back.
///
/// `{uri}` is Caddy's placeholder for the request's path and query, so
/// `https://aiwisent.com/pricing?a=1` lands on
/// `https://wisent-app.com/pricing?a=1` — the same thing the Vercel rewrite
/// these replace did with `/:path*`.
pub(in crate::cli::web) fn redirect(
    hostname: &str,
    target: &str,
) -> Result<(String, String), CmdError> {
    if !config::is_public_hostname(hostname) {
        return Err(CmdError::click(format!(
            "{hostname:?} is not a public host name, so no certificate can be ordered for it \
             and it is not written into the edge's configuration"
        )));
    }
    if !config::is_redirect_target(target) {
        return Err(CmdError::click(format!(
            "{target:?} is not a redirect target: an https URL with a host, no query or fragment, \
             and no trailing slash"
        )));
    }
    Ok((hostname.to_string(), format!("redir {target}{{uri}} 308")))
}

/// One route: the public hostname the edge terminates, and the directive that
/// answers it — a `reverse_proxy` at the upstream behind it.
///
/// The second half of the pair is the rendered directive rather than the bare
/// upstream, because a site block is not always a proxy: a redirect product
/// renders `redir` instead, and one shape for both keeps the renderer from
/// having to know which kind it is looking at.
///
/// The upstream is reached over the tailnet by the host's own tailnet name —
/// the registry target name, which MagicDNS resolves through the search domain
/// tailscaled installs. Not the `*.ts.net` fully qualified form: hard-coding
/// one tailnet's domain into the generated configuration would break the day
/// the fleet gains a second tailnet, and not a loopback address, because the
/// unit runs on a different host from the proxy.
///
/// Both halves are checked because both end up in a generated configuration
/// file: a hostname carrying a space or a brace would produce a Caddyfile that
/// either fails to parse or, worse, parses into something else. The
/// configuration plane already refuses a declaration whose hostname is not a
/// public host name, so in practice this guards the values that reach here
/// from anywhere but a validated declaration.
pub(in crate::cli::web) fn route(
    hostname: &str,
    host: &str,
    port: u16,
) -> Result<(String, String), CmdError> {
    if !config::is_public_hostname(hostname) {
        return Err(CmdError::click(format!(
            "{hostname:?} is not a public host name, so no certificate can be ordered for it \
             and it is not written into the edge's configuration"
        )));
    }
    let tailnet_name = !host.is_empty()
        && host.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.')
        });
    if !tailnet_name {
        return Err(CmdError::click(format!(
            "{host:?} is not a tailnet host name, so {hostname} has no upstream the edge can \
             forward to"
        )));
    }
    if port == 0 {
        return Err(CmdError::click(format!(
            "{hostname} declares port 0, which nothing listens on"
        )));
    }
    Ok((
        hostname.to_string(),
        format!("reverse_proxy http://{host}:{port}"),
    ))
}
