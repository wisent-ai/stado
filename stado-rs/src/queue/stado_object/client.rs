//! The constructor and the pooled HTTPS client behind it.
//!
//! [`StadoObjectBackend::new`] validates the store URL and the token file
//! before anything is sent, and the client it binds is shared per origin host
//! and CA configuration so the connection pool outlives the per-call
//! constructors.

use reqwest::{Client, Url};

use crate::remote::object_store::ObjectRef;
use crate::queue::StorageError;

use super::StadoObjectBackend;

impl StadoObjectBackend {
    pub fn new(
        base_url: &str,
        namespace: &str,
        token_file: &str,
        ca_file: &str,
    ) -> Result<Self, StorageError> {
        let mut base_url = Url::parse(base_url.trim())
            .map_err(|error| StorageError::Other(format!("invalid Stado storage URL: {error}")))?;
        let host = base_url.host_str().unwrap_or_default().to_ascii_lowercase();
        let loopback = matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1");
        // Tailscale encrypts traffic before it leaves the host. Its fixed CGNAT
        // and ULA ranges are therefore an authenticated private transport, not
        // clear-text Internet HTTP. Keep names out of this exception: an IP
        // literal makes the transport boundary explicit and cannot be rebound
        // by DNS.
        let tailnet = host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| match address {
                std::net::IpAddr::V4(address) => {
                    let octets = address.octets();
                    octets[0] == 100 && (64..=127).contains(&octets[1])
                }
                std::net::IpAddr::V6(address) => {
                    let segments = address.segments();
                    segments[0] == 0xfd7a && segments[1] == 0x115c && segments[2] == 0xa1e0
                }
            });
        if (base_url.scheme() != "https" && !(base_url.scheme() == "http" && (loopback || tailnet)))
            || !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
            || !matches!(base_url.path(), "" | "/")
        {
            return Err(StorageError::Other(
                "Stado storage URL must be an HTTPS origin or authenticated HTTP loopback/tailnet IP"
                    .to_string(),
            ));
        }
        base_url.set_path("");
        ObjectRef::new(namespace, "configuration-check")?;
        let token_path = crate::config_file::expand_tilde(token_file);
        let metadata = std::fs::symlink_metadata(&token_path).map_err(|error| {
            StorageError::Auth(format!(
                "cannot inspect Stado storage token file {}: {error}",
                token_path.display()
            ))
        })?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            return Err(StorageError::Auth(format!(
                "Stado storage token file must be a regular file: {}",
                token_path.display()
            )));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(StorageError::Auth(format!(
                    "Stado storage token file must be owner-only (chmod 600): {}",
                    token_path.display()
                )));
            }
        }
        let token = std::fs::read_to_string(&token_path)
            .map_err(|error| {
                StorageError::Auth(format!(
                    "cannot read Stado storage token file {}: {error}",
                    token_path.display()
                ))
            })?
            .trim()
            .to_string();
        if token.is_empty()
            || token
                .chars()
                .any(|character| matches!(character, '\r' | '\n'))
        {
            return Err(StorageError::Auth(
                "Stado storage token file is empty or malformed".to_string(),
            ));
        }
        let client = Self::shared_client(&host, ca_file)?;
        Ok(Self {
            base_url,
            namespace: namespace.to_string(),
            token,
            client,
        })
    }

    /// One pooled client per origin host and CA configuration.
    ///
    /// `reqwest::Client` owns the connection pool, and this backend used to
    /// build a fresh one in every constructor. Nothing here constructs once:
    /// `JobStorage::new()` is called per janitor pass, per gates read, per CLI
    /// invocation, and each of those was a pool with one connection in it that
    /// was dropped at the end. The store's own socket table showed the result
    /// on 2026-09-03 — 1,388 `TIME_WAIT` against `127.0.0.1:8765` beside 96
    /// `ESTABLISHED`, with the object API pinned at 95.9% of a core — and a
    /// read that has to queue behind a thousand fresh handshakes is a read
    /// that takes 639 s, which is the latency that starves the agent loop.
    ///
    /// Cloning a `Client` shares its pool, so every backend built in this
    /// process now reuses connections. The CA and origin host select the
    /// client's certificate trust and tailnet address pin. Tokens remain
    /// per-request headers and never belong to the pool key.
    fn shared_client(host: &str, ca_file: &str) -> Result<Client, StorageError> {
        static CLIENTS: std::sync::LazyLock<
            std::sync::Mutex<std::collections::HashMap<String, Client>>,
        > = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));
        let key = format!("{}|{host}", ca_file.trim());
        let mut clients = CLIENTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(client) = clients.get(&key) {
            return Ok(client.clone());
        }
        let client = Self::client(host, ca_file)?;
        clients.insert(key, client.clone());
        Ok(client)
    }

    /// The HTTPS client, trusting the configured private authority in addition to
    /// the system roots.
    ///
    /// A default client carries only the operating system's trust store, so an
    /// object API published on the fleet's tailnet -- signed by the tailnet's own
    /// authority -- fails during the handshake. `reqwest` surfaces that as the
    /// opaque "error sending request", which reads like the host is down rather
    /// than like this process was never told whom to trust. `storage.stado.ca_file`
    /// was already in the deployed configuration and no code path read it, so the
    /// only URL that ever worked was a loopback one and every host quietly
    /// addressed its own store instead of the fleet's.
    ///
    /// The certificate is added, never substituted: publicly signed endpoints keep
    /// working, and this cannot become a way to disable verification.
    fn client(host: &str, ca_file: &str) -> Result<Client, StorageError> {
        // Bounded like `fleet_https_client`, and for the same incident: a
        // release submit's queue write to the fleet store held one ESTABLISHED
        // connection for eight minutes with no error and no progress, because
        // this client had no timeout at all. The store is on the tailnet or the
        // same machine; five minutes covers a multi-megabyte artifact there, and
        // a hang converted into a named error reaches callers that already
        // handle storage failures.
        let mut builder = Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            // Sharing the client is what makes a pool possible; these three
            // state what the pool is for. The idle timeout is the contract
            // with the object API, which holds a reused connection for 120 s
            // (`Dashboard::KEEP_ALIVE_IDLE`): this side must retire the socket
            // FIRST, because a connection retired by the server between the
            // pool checkout and the write fails or re-dials -- the cost this
            // sharing exists to remove. 90 s against the server's 120 s leaves
            // a 30 s margin and is also reqwest's own default, so the value is
            // unchanged and only its reason is now written down.
            .pool_idle_timeout(std::time::Duration::from_secs(90))
            // A tick reads a handful of objects; eight warm connections per
            // host serve that with room for the concurrent janitor read,
            // instead of reqwest's unbounded idle set.
            .pool_max_idle_per_host(8)
            // A pooled connection dropped silently -- by the tailnet, by a
            // NAT table, by a service restart -- is otherwise discovered only
            // when a request is written into it, which surfaces as an
            // occasional failed object operation rather than a clean re-dial.
            .tcp_keepalive(std::time::Duration::from_secs(60));
        if let Some(address) = crate::remote::tailnet::address_of(host) {
            // Use the same tailnet map as the artifact client. The hostname
            // remains unchanged for SNI and certificate verification.
            builder = builder.resolve(host, std::net::SocketAddr::new(address, 0));
        }
        let ca_file = ca_file.trim();
        if ca_file.is_empty() {
            return builder.build().map_err(|error| {
                StorageError::Other(format!("cannot build Stado storage client: {error}"))
            });
        }
        let path = crate::config_file::expand_tilde(ca_file);
        let pem = std::fs::read(&path).map_err(|error| {
            StorageError::Other(format!(
                "cannot read Stado storage CA file {}: {error}",
                path.display()
            ))
        })?;
        let certificate = reqwest::Certificate::from_pem(&pem).map_err(|error| {
            StorageError::Other(format!(
                "Stado storage CA file {} is not a PEM certificate: {error}",
                path.display()
            ))
        })?;
        builder
            .add_root_certificate(certificate)
            .build()
            .map_err(|error| {
                StorageError::Other(format!("cannot build Stado storage HTTPS client: {error}"))
            })
    }
}
