//! The constructor and the pooled HTTPS client behind it.
//!
//! [`StadoObjectBackend::new`] validates the store URL and the token file
//! before anything is sent, and the client it binds is shared per origin host
//! and CA configuration so the connection pool outlives the per-call
//! constructors.

use reqwest::{Client, Url};

use crate::queue::StorageError;
use crate::remote::object_store::ObjectRef;

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
        let Some(host) = base_url.host_str().map(str::to_ascii_lowercase) else {
            return Err(StorageError::Other(format!(
                "Stado storage URL {base_url} names no host"
            )));
        };
        let Some(port) = base_url.port_or_known_default() else {
            return Err(StorageError::Other(format!(
                "Stado storage URL {base_url} names no port and its scheme has none"
            )));
        };
        let loopback = crate::primitives::http::origin::is_loopback_host(&base_url);
        // Tailscale encrypts traffic before it leaves the host, so plain HTTP
        // to a tailnet node is an authenticated private transport. Which
        // addresses are tailnet nodes is the tailnet's own answer, never an
        // address range: the shared CGNAT space is a carrier's too. Names stay
        // out of this exception: an IP literal makes the transport boundary
        // explicit and cannot be rebound by DNS.
        let tailnet = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .is_ok_and(crate::remote::tailnet::is_node_address);
        if !crate::primitives::http::origin::is_secure_or_local_plain(
            &base_url,
            loopback || tailnet,
        ) || !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
            || !matches!(base_url.path(), "" | "/")
        {
            return Err(StorageError::Other(format!(
                "Stado storage URL {base_url} must be an HTTPS origin, or authenticated HTTP to \
                 loopback or to an address this machine's tailnet lists for one of its nodes \
                 (tailscale status --json), with no credentials, query, fragment or path"
            )));
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
            use nix::sys::stat::Mode;
            use std::os::unix::fs::PermissionsExt;
            let mode = Mode::from_bits_truncate(metadata.permissions().mode() as nix::libc::mode_t);
            if mode.intersects(Mode::S_IRWXG | Mode::S_IRWXO) {
                return Err(StorageError::Auth(format!(
                    "Stado storage token file must be owner-only (chmod go-rwx): {}",
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
        let client = Self::shared_client(&host, port, ca_file)?;
        Ok(Self {
            base_url,
            namespace: namespace.to_string(),
            token,
            client,
        })
    }

    /// One client per origin host, port and CA configuration.
    ///
    /// `reqwest::Client` owns the connection pool, and this backend used to
    /// build a fresh one in every constructor. Nothing here constructs once:
    /// `JobStorage::new()` is called per janitor pass, per gates read, per CLI
    /// invocation, and each of those was a pool with one connection in it that
    /// was dropped at the end. The store's own socket table shows the result
    /// — thousands of `TIME_WAIT` against the loopback port beside a few
    /// dozen `ESTABLISHED`, with the object API pinned near a full core —
    /// and a read that has to queue behind a thousand fresh handshakes is a
    /// read that takes minutes, which is the latency that starves the agent
    /// loop.
    ///
    /// One client per origin is built once per process and cloned, so its
    /// TLS configuration, certificate trust and tailnet address pin are read
    /// once; each request still opens its own connection ([`Self::client`]).
    /// Tokens remain per-request headers and never belong to the client key.
    fn shared_client(host: &str, port: u16, ca_file: &str) -> Result<Client, StorageError> {
        static CLIENTS: std::sync::LazyLock<
            std::sync::Mutex<std::collections::HashMap<String, Client>>,
        > = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));
        let key = format!("{}|{host}|{port}", ca_file.trim());
        let mut clients = match CLIENTS.lock() {
            Ok(clients) => clients,
            // A panic elsewhere left the map as it was; every entry in it is
            // a finished client, so the map is still sound to read.
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(client) = clients.get(&key) {
            return Ok(client.clone());
        }
        let client = Self::client(host, port, ca_file)?;
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
    fn client(host: &str, port: u16, ca_file: &str) -> Result<Client, StorageError> {
        // Shared for its TLS configuration, but no connection is reused: the
        // store is usually reached through a local forward whose upstream
        // channel can end (a store restart, a lost session) while the
        // client's socket to the forward stays open, and a request written to
        // such a pooled socket waited for an answer that never came, stalling
        // the agent tick. Every request says `Connection: close` (Stado's
        // reqwest is built without HTTP/2, so every request is HTTP/1.1 and
        // honours it), so the connection ends with its response and the next
        // request reaches the store that is serving now or fails with that
        // connection's error.
        let mut close = reqwest::header::HeaderMap::new();
        close.insert(
            reqwest::header::CONNECTION,
            reqwest::header::HeaderValue::from_static("close"),
        );
        let mut builder = Client::builder().default_headers(close);
        if let Some(address) = crate::remote::tailnet::address_of(host) {
            // Use the same tailnet map as the artifact client. The hostname
            // remains unchanged for SNI and certificate verification; the
            // port is the store URL's own.
            builder = builder.resolve(host, std::net::SocketAddr::new(address, port));
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
