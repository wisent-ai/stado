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
        if (base_url.scheme() != "https" && !(base_url.scheme() == "http" && loopback))
            || !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
            || !matches!(base_url.path(), "" | "/")
        {
            return Err(StorageError::Other(
                "Stado storage URL must be an HTTPS origin or authenticated HTTP loopback"
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
        let ca_file = ca_file.trim();
        if ca_file.is_empty() {
            return Ok(Client::new());
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
        Client::builder()
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
        let response = self
            .request(Method::PUT, self.object_url(path, &options)?)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            // Reqwest omits Content-Length for an empty Vec body. The object
            // endpoint requires the header even when the payload is zero bytes,
            // so empty logs and artifacts must declare their length explicitly.
            .header(reqwest::header::CONTENT_LENGTH, content.len())
            .body(content)
            .send()
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
        let response = self.request(Method::GET, url).send().await?;
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
}

#[async_trait]
impl BlobBackend for StadoObjectBackend {
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
        let response = self
            .request(Method::GET, self.object_url(path, &[])?)
            .send()
            .await?;
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
        let response = self
            .request(
                Method::GET,
                self.object_url(path, &[("versioned", "true")])?,
            )
            .send()
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
        let response = self
            .request(
                Method::PUT,
                self.object_url(path, &[("if_version", expected_version)])?,
            )
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(content.to_string())
            .send()
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
        let response = self
            .request(Method::DELETE, self.object_url(path, &[])?)
            .send()
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
        let response = self
            .request(
                Method::PUT,
                self.object_url(path, &[("metadata_only", "true")])?,
            )
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .json(kv)
            .send()
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
        ObjectRef::new(&self.namespace, &format!("{prefix}sentinel"))?;
        let mut url = self.url("/api/object/list");
        url.query_pairs_mut()
            .append_pair("namespace", &self.namespace)
            .append_pair("prefix", prefix);
        let response = self.request(Method::GET, url).send().await?;
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
