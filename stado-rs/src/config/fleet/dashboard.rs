//! API listener bind, port, proxy trust and deployment identity.

use std::sync::LazyLock;

use crate::config_file::resolve as cfg;

static DASHBOARD_BIND: LazyLock<String> =
    LazyLock::new(|| cfg("WC_DASHBOARD_BIND", "dashboard.bind", "127.0.0.1"));
static DASHBOARD_PORT: LazyLock<String> = LazyLock::new(|| {
    cfg("WC_DASHBOARD_PORT", "dashboard.port", "")
        .trim()
        .to_string()
});
static DASHBOARD_TRUST_HTTPS_PROXY: LazyLock<bool> = LazyLock::new(|| {
    let value = cfg(
        "WC_DASHBOARD_TRUST_HTTPS_PROXY",
        "dashboard.trust_https_proxy",
        "false",
    );
    let value = value.trim();
    value == "1"
        || value.eq_ignore_ascii_case("true")
        || value.eq_ignore_ascii_case("yes")
        || value.eq_ignore_ascii_case("on")
});

/// API listener bind address (env `WC_DASHBOARD_BIND`). Azure
/// cutover keeps this on loopback behind the TLS reverse proxy; never bind a
/// private dashboard route directly to a public interface.
pub fn dashboard_bind() -> &'static str {
    DASHBOARD_BIND.as_str()
}

/// API listener port the deployment declares (env `WC_DASHBOARD_PORT`, config
/// key `dashboard.port`), or `None` when it declares none: there is no
/// built-in port. `serve --port` declares it on the command line instead;
/// `0` asks the system for a free port, which the listener announces.
pub fn dashboard_port() -> Option<String> {
    let port = DASHBOARD_PORT.as_str();
    (!port.is_empty()).then(|| port.to_string())
}

/// Whether the loopback listener accepts host authorities supplied by an HTTPS
/// reverse proxy.
pub fn dashboard_trust_https_proxy() -> bool {
    *DASHBOARD_TRUST_HTTPS_PROXY
}

/// Deployment identity (env `STADO_DEPLOYMENT_ID`, config
/// key `deployment.id`), trimmed. A bound deployment implies an authenticated
/// HTTPS reverse proxy fronts the loopback listener.
///
/// Read per call so a process-level override remains dynamic; the config
/// file itself is cached by [`crate::config_file`].
pub fn stado_deployment_id() -> String {
    cfg("STADO_DEPLOYMENT_ID", "deployment.id", "")
        .trim()
        .to_string()
}
