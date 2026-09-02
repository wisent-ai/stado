//! Shared queue storage through Stado's authenticated object API.
//!
//! Remote local workers cannot use `LocalBackend`: its filesystem is private to
//! one host. This adapter keeps queue paths provider-neutral while the control
//! plane remains the sole owner of the concrete backing store.

use std::collections::BTreeMap;
use std::path::Path;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::{Client, Method, Response, StatusCode, Url};
use serde::Deserialize;

use super::{BlobBackend, BlobInfo, StorageError, VersionedText};
use crate::object_store::ObjectRef;

const VERSION_HEADER: &str = "x-stado-version";

#[derive(Debug)]
pub struct StadoObjectBackend {
    base_url: Url,
    namespace: String,
    token: String,
    client: Client,
}

#[derive(Debug, Deserialize)]
struct ObjectList {
    objects: Vec<ObjectDescriptor>,
}

#[derive(Debug, Deserialize)]
struct ObjectDescriptor {
    key: String,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    metadata: BTreeMap<String, String>,
}

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
        Ok(Self {
            base_url,
            namespace: namespace.to_string(),
            token,
            client: Self::client(ca_file)?,
        })
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
    fn client(ca_file: &str) -> Result<Client, StorageError> {
        // Bounded like `fleet_https_client`, and for the same incident: a
        // release submit's queue write to the fleet store held one ESTABLISHED
        // connection for eight minutes with no error and no progress, because
        // this client had no timeout at all. The store is on the tailnet or the
        // same machine; five minutes covers a multi-megabyte artifact there, and
        // a hang converted into a named error reaches callers that already
        // handle storage failures.
        let builder = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(300));
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

    fn object(&self, path: &str) -> Result<ObjectRef, StorageError> {
        ObjectRef::new(&self.namespace, path)
    }

    fn url(&self, endpoint: &str) -> Url {
        let mut url = self.base_url.clone();
        url.set_path(endpoint);
        url
    }

    fn object_url(&self, path: &str, options: &[(&str, &str)]) -> Result<Url, StorageError> {
        let object = self.object(path)?;
        let mut url = self.url("/api/object");
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("uri", &object.to_string());
            for (name, value) in options {
                query.append_pair(name, value);
            }
        }
        Ok(url)
    }

    fn request(&self, method: Method, url: Url) -> reqwest::RequestBuilder {
        self.client
            .request(method, url)
            .bearer_auth(&self.token)
            .header(reqwest::header::ACCEPT, "application/json")
    }

    /// The body the object gateway answers with while its authorization
    /// boundary is closed. Matched on the body and not on the status alone:
    /// `503` is also how an ingress in front of the gateway says it has no
    /// upstream, and that is not a window anything should wait out.
    const BOUNDARY_UNAVAILABLE_BODY: &'static str = "object authorization unavailable";

    /// Attempts, including the first, before a closed boundary is reported.
    const BOUNDARY_ATTEMPTS: usize = 6;

    /// Send a request the gateway may refuse while it revalidates its grants.
    ///
    /// The object gateway reads its verifier grants from Skarbiec and answers
    /// `503 {"error":"object authorization unavailable"}` for as long as that
    /// read is in flight — the same mechanism the dashboard logs as
    /// "integration authorization boundary is closed; revalidating inline". It
    /// is a window, not a verdict. `deploy.yml`'s Linux publisher has ridden it
    /// out with twelve tries for as long as it has existed ("The writer may
    /// briefly reload authorization"), and every other caller in the fleet died
    /// on the first refusal: the `weles-worker 0.5.26` submission ended at
    /// 2026-08-29T23:02:53Z on exactly this body, three seconds after the same
    /// client had read a storage state successfully through the same gateway.
    ///
    /// Only that one status-and-body pair is retried, and only
    /// [`Self::BOUNDARY_ATTEMPTS`] times with a linear backoff. Any other body
    /// under `503`, and every other status, reaches the caller unchanged — a
    /// gateway that is genuinely unauthorized must still say so on the first
    /// answer.
    async fn send_through_boundary(
        builder: reqwest::RequestBuilder,
    ) -> Result<Response, StorageError> {
        let Some(mut candidate) = builder.try_clone() else {
            // A streaming body cannot be replayed, so there is nothing to retry
            // with; the caller gets the gateway's first answer.
            return Ok(builder.send().await?);
        };
        for attempt in 1..=Self::BOUNDARY_ATTEMPTS {
            let response = candidate.send().await?;
            if response.status() != StatusCode::SERVICE_UNAVAILABLE
                || attempt == Self::BOUNDARY_ATTEMPTS
            {
                return Ok(response);
            }
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            if !body.contains(Self::BOUNDARY_UNAVAILABLE_BODY) {
                return Err(StorageError::Stado { status, body });
            }
            let Some(next) = builder.try_clone() else {
                return Err(StorageError::Stado { status, body });
            };
            candidate = next;
            tokio::time::sleep(std::time::Duration::from_secs(attempt as u64)).await;
        }
        // The loop returns on its last attempt, so this is unreachable while
        // BOUNDARY_ATTEMPTS is non-zero; stated rather than left to a panic.
        Err(StorageError::Other(
            "Stado object API boundary retry made no attempt".to_string(),
        ))
    }

    async fn response_error(response: Response) -> StorageError {
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        StorageError::Stado { status, body }
    }

    async fn upload(
        &self,
        path: &str,
        content: Vec<u8>,
        if_absent: bool,
    ) -> Result<bool, StorageError> {
        let options = if if_absent {
            vec![("if_absent", "true")]
        } else {
            Vec::new()
        };
        let response = Self::send_through_boundary(
            self.request(Method::PUT, self.object_url(path, &options)?)
                .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                // Reqwest omits Content-Length for an empty Vec body. The object
                // endpoint requires the header even when the payload is zero bytes,
                // so empty logs and artifacts must declare their length explicitly.
                .header(reqwest::header::CONTENT_LENGTH, content.len())
                .body(content),
        )
        .await?;
        if matches!(
            response.status(),
            StatusCode::CONFLICT | StatusCode::PRECONDITION_FAILED
        ) {
            return Ok(false);
        }
        if !response.status().is_success() {
            return Err(Self::response_error(response).await);
        }
        Ok(true)
    }

    async fn stat(&self, path: &str) -> Result<Option<ObjectDescriptor>, StorageError> {
        let object = self.object(path)?;
        let mut url = self.url("/api/object/stat");
        url.query_pairs_mut()
            .append_pair("uri", &object.to_string());
        let response = Self::send_through_boundary(self.request(Method::GET, url)).await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(Self::response_error(response).await);
        }
        #[derive(Deserialize)]
        struct StatResponse {
            uri: String,
            #[serde(default)]
            size: Option<u64>,
            #[serde(default)]
            updated_at: Option<String>,
            #[serde(default)]
            metadata: BTreeMap<String, String>,
        }
        let stat: StatResponse = response.json().await?;
        let key = ObjectRef::parse(&stat.uri)?.key().to_string();
        Ok(Some(ObjectDescriptor {
            key,
            size: stat.size,
            updated_at: stat.updated_at,
            metadata: stat.metadata,
        }))
    }

    fn parse_updated(value: Option<String>) -> Result<Option<DateTime<Utc>>, StorageError> {
        value
            .map(|value| {
                DateTime::parse_from_rfc3339(&value)
                    .map(|timestamp| timestamp.with_timezone(&Utc))
                    .map_err(|error| {
                        StorageError::Other(format!(
                            "Stado object API returned invalid updated_at {value:?}: {error}"
                        ))
                    })
            })
            .transpose()
    }

    /// The listing URL for `prefix`, with the prefix validation every listing
    /// route needs: the gateway takes a namespace and a prefix and nothing
    /// else, so both listings address exactly the same endpoint.
    fn list_url(&self, prefix: &str) -> Result<Url, StorageError> {
        ObjectRef::new(&self.namespace, &format!("{prefix}sentinel"))?;
        let mut url = self.url("/api/object/list");
        url.query_pairs_mut()
            .append_pair("namespace", &self.namespace)
            .append_pair("prefix", prefix);
        Ok(url)
    }
}

#[async_trait]
impl BlobBackend for StadoObjectBackend {
    /// The object API is addressed by the bare key: [`Self::object`] builds
    /// `ObjectRef::new(&self.namespace, path)`, so handing it a qualified
    /// store path asks for `<namespace>/ecosystem/<namespace>/<key>`.
    fn blob_path(&self, object: &ObjectRef) -> String {
        object.key().to_string()
    }

    /// The list route takes `namespace` and `prefix` as separate query
    /// parameters, so the prefix it wants is the bare one too. An empty
    /// prefix is the whole namespace and is passed through as empty rather
    /// than validated as a key.
    fn blob_prefix(&self, _namespace: &str, prefix: &str) -> Result<String, StorageError> {
        Ok(prefix.trim_matches('/').to_string())
    }

    async fn upload_text(&self, path: &str, content: &str) -> Result<(), StorageError> {
        self.upload(path, content.as_bytes().to_vec(), false)
            .await
            .map(|_| ())
    }

    async fn upload_bytes(&self, path: &str, content: &[u8]) -> Result<(), StorageError> {
        self.upload(path, content.to_vec(), false).await.map(|_| ())
    }

    async fn download_text(&self, path: &str) -> Result<Option<String>, StorageError> {
        let Some(bytes) = self.download_bytes(path).await? else {
            return Ok(None);
        };
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|error| StorageError::Other(format!("invalid UTF-8 in {path}: {error}")))
    }

    async fn download_bytes(&self, path: &str) -> Result<Option<Vec<u8>>, StorageError> {
        let response =
            Self::send_through_boundary(self.request(Method::GET, self.object_url(path, &[])?))
                .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(Self::response_error(response).await);
        }
        Ok(Some(response.bytes().await?.to_vec()))
    }

    /// One `stado://releases/...` object off the fleet's public release
    /// channel.
    ///
    /// The plain object route answers for the store's configured namespace,
    /// so reading a cross-namespace release URI through `download_bytes`
    /// quietly asks for `<namespace>/releases/...` and reports the software
    /// artifact absent — which is how every fleet delivery of 0.7.6 failed
    /// while the archive sat published. `/api/release/object` is the route
    /// the channel itself serves those bytes on.
    async fn download_release(&self, uri: &str) -> Result<Option<Vec<u8>>, StorageError> {
        let mut url = self.url("/api/release/object");
        url.query_pairs_mut().append_pair("uri", uri);
        let response = Self::send_through_boundary(self.request(Method::GET, url)).await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(Self::response_error(response).await);
        }
        Ok(Some(response.bytes().await?.to_vec()))
    }

    async fn download_to_filename(&self, path: &str, dest: &Path) -> Result<bool, StorageError> {
        let Some(bytes) = self.download_bytes(path).await? else {
            return Ok(false);
        };
        tokio::fs::write(dest, bytes).await?;
        Ok(true)
    }

    async fn upload_text_if_absent(&self, path: &str, content: &str) -> Result<bool, StorageError> {
        self.upload(path, content.as_bytes().to_vec(), true).await
    }

    async fn upload_file_if_absent(
        &self,
        path: &str,
        local_file: &Path,
    ) -> Result<bool, StorageError> {
        self.upload(path, tokio::fs::read(local_file).await?, true)
            .await
    }

    async fn download_text_versioned(
        &self,
        path: &str,
    ) -> Result<Option<VersionedText>, StorageError> {
        let response = Self::send_through_boundary(self.request(
            Method::GET,
            self.object_url(path, &[("versioned", "true")])?,
        ))
        .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(Self::response_error(response).await);
        }
        let version = response
            .headers()
            .get(VERSION_HEADER)
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                StorageError::Other(format!(
                    "Stado object API omitted {VERSION_HEADER} for {path}"
                ))
            })?
            .to_string();
        let bytes = response.bytes().await?.to_vec();
        let content = String::from_utf8(bytes)
            .map_err(|error| StorageError::Other(format!("invalid UTF-8 in {path}: {error}")))?;
        Ok(Some(VersionedText { content, version }))
    }

    async fn compare_and_swap_text(
        &self,
        path: &str,
        expected_version: &str,
        content: &str,
    ) -> Result<String, StorageError> {
        let response = Self::send_through_boundary(
            self.request(
                Method::PUT,
                self.object_url(path, &[("if_version", expected_version)])?,
            )
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(content.to_string()),
        )
        .await?;
        if response.status() == StatusCode::CONFLICT {
            return Err(StorageError::StorageConflict(format!(
                "Stado storage version changed for {path}"
            )));
        }
        if response.status() == StatusCode::NOT_FOUND {
            return Err(StorageError::NotFound(path.to_string()));
        }
        if !response.status().is_success() {
            return Err(Self::response_error(response).await);
        }
        #[derive(Deserialize)]
        struct CasResponse {
            version: String,
        }
        let payload: CasResponse = response.json().await?;
        if payload.version.is_empty() {
            return Err(StorageError::Other(format!(
                "Stado object API returned an empty version for {path}"
            )));
        }
        Ok(payload.version)
    }

    async fn delete(&self, path: &str) -> Result<(), StorageError> {
        let response =
            Self::send_through_boundary(self.request(Method::DELETE, self.object_url(path, &[])?))
                .await?;
        if response.status() == StatusCode::NOT_FOUND || response.status().is_success() {
            return Ok(());
        }
        Err(Self::response_error(response).await)
    }

    async fn exists(&self, path: &str) -> Result<bool, StorageError> {
        Ok(self.stat(path).await?.is_some())
    }

    async fn list_paths(
        &self,
        prefix: &str,
        oldest_first: usize,
    ) -> Result<Vec<String>, StorageError> {
        let mut blobs = self.list_blobs_with_meta(prefix).await?;
        if oldest_first > 0 {
            blobs.sort_by(|left, right| {
                left.updated
                    .cmp(&right.updated)
                    .then(left.name.cmp(&right.name))
            });
            blobs.truncate(oldest_first);
        } else {
            blobs.sort_by(|left, right| left.name.cmp(&right.name));
        }
        Ok(blobs.into_iter().map(|blob| blob.name).collect())
    }

    /// The gateway has no server-side cursor to use: `/api/object/list` takes
    /// a namespace and a prefix, and answers with the whole authorized prefix
    /// as descriptors — there is no offset, no limit, and no name-only
    /// projection to ask for, and inventing one would page against a server
    /// that ignores it and silently return the wrong window. So the cut stays
    /// on this side and the round-trip is the same one [`Self::list_paths`]
    /// would make.
    ///
    /// What the override does buy is the metadata work behind that response.
    /// The default reaches `list_paths`, which builds a [`BlobInfo`] per
    /// object and parses every RFC 3339 `updated_at` in the prefix — 14k
    /// timestamp parses and 14k metadata maps to answer a request for a few
    /// names that need none of it. Worse, that parse is fallible, so one
    /// malformed timestamp anywhere under the prefix fails a page that would
    /// never have returned the object. Reading only the keys is both cheaper
    /// and harder to break, and this is the single place to add a cursor when
    /// the gateway grows one.
    async fn list_page(
        &self,
        prefix: &str,
        start_after: &str,
        limit: usize,
    ) -> Result<Vec<String>, StorageError> {
        let response =
            Self::send_through_boundary(self.request(Method::GET, self.list_url(prefix)?)).await?;
        if !response.status().is_success() {
            return Err(Self::response_error(response).await);
        }
        let payload: ObjectList = response.json().await?;
        let mut names: Vec<String> = payload.objects.into_iter().map(|object| object.key).collect();
        names.sort_unstable();
        let cut = names.partition_point(|name| name.as_str() <= start_after);
        names.drain(..cut);
        if limit > 0 {
            names.truncate(limit);
        }
        Ok(names)
    }

    async fn updated_at(&self, path: &str) -> Result<Option<DateTime<Utc>>, StorageError> {
        let Some(stat) = self.stat(path).await? else {
            return Ok(None);
        };
        Self::parse_updated(stat.updated_at)
    }

    async fn set_metadata(
        &self,
        path: &str,
        kv: &BTreeMap<String, String>,
    ) -> Result<(), StorageError> {
        let response = Self::send_through_boundary(
            self.request(
                Method::PUT,
                self.object_url(path, &[("metadata_only", "true")])?,
            )
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(kv),
        )
        .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        if !response.status().is_success() {
            return Err(Self::response_error(response).await);
        }
        Ok(())
    }

    async fn list_blobs_with_meta(&self, prefix: &str) -> Result<Vec<BlobInfo>, StorageError> {
        let url = self.list_url(prefix)?;
        let response = Self::send_through_boundary(self.request(Method::GET, url)).await?;
        if !response.status().is_success() {
            return Err(Self::response_error(response).await);
        }
        let payload: ObjectList = response.json().await?;
        payload
            .objects
            .into_iter()
            .map(|object| {
                Ok(BlobInfo {
                    name: object.key,
                    updated: Self::parse_updated(object.updated_at)?,
                    size: object.size,
                    metadata: object.metadata,
                })
            })
            .collect()
    }
}
