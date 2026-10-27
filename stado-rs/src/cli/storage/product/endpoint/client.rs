//! One HTTPS client per process and configuration, and the hosts it pins.

use crate::cli::storage::*;

/// One HTTPS client that trusts what `storage.stado.ca_file` names.
///
/// The queue backend already loads that certificate; callers that built their
/// own client did not, so the moment the fleet's control plane moved from
/// loopback to a tailnet HTTPS origin they failed with "error sending
/// request" -- which reads like the host is down rather than like this
/// process was never told whom to trust.
///
/// Built once per process, per configuration. `reqwest::Client` owns the
/// connection pool, and this was called per object operation --
/// `RemoteObjectApi::configured()` runs on every get, stat, list, put,
/// get_versioned, put_if_version and delete, and the beacon publisher, the
/// doctor and the host-recovery release path each call it directly. A client
/// per call is a pool of one connection thrown away, which is how a store
/// ends up with 1,059 sockets in `TIME_WAIT` beside 41 established and an
/// object API pinned at 99.8% of a core, and a read that queues behind those
/// handshakes is the read that starves the agent loop. Same fix, and the same
/// reasoning, as `queue::stado_object::StadoObjectBackend::shared_client`.
///
/// Keyed by the inputs the client is built from -- the CA file and the
/// resolved origin hosts -- so a configuration change still produces a new
/// client rather than reusing one that trusts the wrong authority.
pub(crate) fn fleet_https_client() -> Result<reqwest::Client, CmdError> {
    static CLIENTS: std::sync::LazyLock<
        std::sync::Mutex<std::collections::HashMap<String, reqwest::Client>>,
    > = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    // The cache is taken even after a panic poisoned its lock: it holds
    // built clients only, and a thread that stopped mid-insert leaves at
    // worst one client to build again.
    fn clients(
    ) -> std::sync::MutexGuard<'static, std::collections::HashMap<String, reqwest::Client>> {
        match CLIENTS.lock() {
            Ok(clients) => clients,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
    let origins: Vec<String> = configured_origins()
        .into_iter()
        .map(|(host, port)| format!("{host}:{port}"))
        .collect();
    let key = format!(
        "{}|{}",
        crate::config::wc_stado_storage_ca_file().trim(),
        origins.join(",")
    );
    if let Some(client) = clients().get(&key) {
        return Ok(client.clone());
    }
    let client = build_fleet_https_client()?;
    clients().insert(key, client.clone());
    Ok(client)
}

fn build_fleet_https_client() -> Result<reqwest::Client, CmdError> {
    // One client per configuration, so connections are reused. A pooled
    // connection stays until the server closes it.
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .pool_idle_timeout(None);
    for (host, port) in configured_origins() {
        // The tailnet states where its own names live. Asking the system
        // resolver about a MagicDNS name is asking a witness that may not have
        // been told: it can answer the public front end once and nothing
        // the next time, while the tailnet address serves the same route
        // at once. SNI and certificate validation still use the
        // name, so this decides the route and never the identity.
        if let Some(address) = crate::remote::tailnet::address_of(&host) {
            builder = builder.resolve(&host, std::net::SocketAddr::new(address, port));
        }
    }
    let ca_file = crate::config::wc_stado_storage_ca_file().trim().to_string();
    if !ca_file.is_empty() {
        let path = crate::config_file::expand_tilde(&ca_file);
        let pem = std::fs::read(&path).map_err(|error| {
            CmdError::click(format!(
                "cannot read storage.stado.ca_file {}: {error}",
                path.display()
            ))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
        let certificate = reqwest::Certificate::from_pem(&pem).map_err(|error| {
            CmdError::click(format!(
                "storage.stado.ca_file {} is not a PEM certificate: {error}",
                path.display()
            ))
            .stating(crate::primitives::failure::FailureCode::Config)
        })?;
        builder = builder.add_root_certificate(certificate);
    }
    builder.build().map_err(CmdError::from)
}

/// Every tailnet host this process may address as a Stado HTTP origin, with
/// the port its URL declares (or its scheme's known default).
///
/// Read from the accessors that own each origin rather than from raw
/// environment variables, so a value configured in `config.json` is pinned
/// exactly like one exported into the process. Malformed values are dropped
/// here and refused where they are used, because this function decides
/// routing and must never be the thing that rejects a configuration.
fn configured_origins() -> Vec<(String, u16)> {
    let mut origins: Vec<(String, u16)> = Vec::new();
    // A health API variable that is unset, or not Unicode, names no origin
    // host; the value is refused where the health API is used, not here.
    let health_api = match std::env::var("STADO_HOST_HEALTH_API_URL") {
        Ok(url) => url,
        Err(std::env::VarError::NotPresent) => String::new(),
        Err(std::env::VarError::NotUnicode(_refused_where_used)) => String::new(),
    };
    // A store URL naming a service this host cannot place names no origin
    // either; the storage backend refuses it with that cause where it is used.
    let storage_url = match crate::config::wc_stado_storage_url() {
        Ok(url) => url,
        Err(_refused_where_used) => String::new(),
    };
    let candidates = [crate::config::stado_api_url(), storage_url, health_api];
    for candidate in candidates {
        let candidate = candidate.trim();
        if candidate.is_empty() {
            continue;
        }
        let Ok(url) = url::Url::parse(candidate) else {
            continue;
        };
        let Some(host) = url.host_str() else { continue };
        // A URL whose scheme has no known port declares none to pin at.
        let Some(port) = url.port_or_known_default() else {
            continue;
        };
        if crate::remote::tailnet::is_magicdns_name(host)
            && !origins.iter().any(|(known, _port)| known == host)
        {
            origins.push((host.to_string(), port));
        }
    }
    origins
}
