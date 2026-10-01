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
    let key = format!(
        "{}|{}",
        crate::config::wc_stado_storage_ca_file().trim(),
        configured_origin_hosts().join(",")
    );
    if let Some(client) = CLIENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&key)
    {
        return Ok(client.clone());
    }
    let client = build_fleet_https_client()?;
    CLIENTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(key, client.clone());
    Ok(client)
}

fn build_fleet_https_client() -> Result<reqwest::Client, CmdError> {
    // One bound, on the whole request. Two bounds that each cover a phase —
    // establishment, and a body that has gone quiet — together bound
    // nothing: a peer that stops answering without ever sending FIN or RST is
    // outside both, and a process can then live for hours holding sockets.
    // When that process holds the disk janitor's exclusive run lock, cleanup
    // completes no pass, `disk_cleanup_stalled` latches, and the host claims
    // nothing. The ceiling below is outside no phase: it covers the call.
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        // The pool contract of
        // `queue::stado_object::StadoObjectBackend::client`, and for the same
        // reason: the object API holds a reused connection for 120 s
        // (`Dashboard::KEEP_ALIVE_IDLE`), so this side retires it first at
        // 90 s and never writes into a socket the server is closing. Eight
        // warm connections per host bound the idle set.
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(8)
        // Prove the peer is still there. A vanished peer leaves an
        // ESTABLISHED socket that reads forever, which is exactly what was
        // measured today; keep-alive probes turn that into an error the
        // caller can act on.
        .tcp_keepalive(Duration::from_secs(60));
    for host in configured_origin_hosts() {
        // The tailnet states where its own names live. Asking the system
        // resolver about a MagicDNS name is asking a witness that may not have
        // been told: on 2026-09-02 it answered the public `ts.net` front end
        // once and nothing the next time, while the tailnet address served the
        // same route in 82 ms. SNI and certificate validation still use the
        // name, so this decides the route and never the identity.
        if let Some(address) = crate::remote::tailnet::address_of(&host) {
            builder = builder.resolve(&host, std::net::SocketAddr::new(address, 0));
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
        })?;
        let certificate = reqwest::Certificate::from_pem(&pem).map_err(|error| {
            CmdError::click(format!(
                "storage.stado.ca_file {} is not a PEM certificate: {error}",
                path.display()
            ))
        })?;
        builder = builder.add_root_certificate(certificate);
    }
    builder.build().map_err(CmdError::from)
}

/// Every host this process may address as a Stado HTTP origin.
///
/// Read from the accessors that own each origin rather than from raw
/// environment variables, so a value configured in `config.json` is pinned
/// exactly like one exported into the process. Malformed values are dropped
/// here and refused where they are used, because this function decides
/// routing and must never be the thing that rejects a configuration.
fn configured_origin_hosts() -> Vec<String> {
    let mut hosts = Vec::new();
    let candidates = [
        crate::config::stado_api_url(),
        crate::config::wc_stado_storage_url().to_string(),
        std::env::var("STADO_HOST_HEALTH_API_URL").unwrap_or_default(),
    ];
    for candidate in candidates {
        let candidate = candidate.trim();
        if candidate.is_empty() {
            continue;
        }
        let Ok(url) = url::Url::parse(candidate) else {
            continue;
        };
        let Some(host) = url.host_str() else { continue };
        if crate::remote::tailnet::is_magicdns_name(host)
            && !hosts.iter().any(|known| known == host)
        {
            hosts.push(host.to_string());
        }
    }
    hosts
}
