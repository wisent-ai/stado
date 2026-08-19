//! Google Cloud Storage backend using the JSON API and scoped token provider.
//!
//! Authentication accepts a GCP managed identity or the `stado-gcp` Skarbiec
//! service-account item; cloud CLI sessions and subprocess fallbacks are not
//! credential sources. GCS generations provide create-if-absent and
//! compare-and-swap semantics. Authorization, transport, and non-precondition
//! provider failures remain observable to callers.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::{Method, StatusCode};

use super::{BlobBackend, BlobInfo, StorageError, VersionedText};

/// Read/write storage OAuth scope and JSON API base.
const STORAGE_SCOPE: &str = "https://www.googleapis.com/auth/devstorage.read_write";
const API_BASE: &str = "https://storage.googleapis.com";

struct Inner {
    client: reqwest::Client,
    bucket: String,
    auth: Arc<dyn gcp_auth::TokenProvider>,
}

/// GCS implementation of [`BlobBackend`]. Cheap to clone.
#[derive(Clone)]
pub struct GcsBackend {
    inner: Arc<Inner>,
}

impl GcsBackend {
    /// Build a backend for `bucket`; authentication failure is terminal.
    pub async fn new(bucket: &str) -> Result<Self, StorageError> {
        let auth = crate::skarbiec::gcp_provider().await.map_err(|err| {
            StorageError::Auth(format!(
                "no scoped GCP credentials found for the GCS backend: {err}"
            ))
        })?;
        Ok(Self {
            inner: Arc::new(Inner {
                client: reqwest::Client::new(),
                bucket: bucket.to_string(),
                auth,
            }),
        })
    }

    /// The bucket this backend is bound to (`config::bucket()` by default).
    pub fn bucket(&self) -> &str {
        &self.inner.bucket
    }

    /// Fresh (cached by gcp_auth until expiry) bearer token.
    async fn token(&self) -> Result<String, StorageError> {
        let token = self
            .inner
            .auth
            .token(&[STORAGE_SCOPE])
            .await
            .map_err(|err| StorageError::Auth(err.to_string()))?;
        Ok(format!("Bearer {}", token.as_str()))
    }

    /// Execute an authenticated request and pass through success responses.
    async fn send(
        &self,
        method: Method,
        url: &str,
        body: Option<(String, Vec<u8>)>,
    ) -> Result<reqwest::Response, StorageError> {
        let mut request = self
            .inner
            .client
            .request(method, url)
            .header(reqwest::header::AUTHORIZATION, self.token().await?);
        if let Some((content_type, bytes)) = body {
            request = request
                .header(reqwest::header::CONTENT_TYPE, content_type)
                .body(bytes);
        }
        Ok(request.send().await?)
    }

    /// Upload `content`, optionally guarded by an `ifGenerationMatch`
    /// precondition ("0" = create-only, generation = CAS).
    async fn upload(
        &self,
        path: &str,
        bytes: Vec<u8>,
        if_generation_match: Option<&str>,
    ) -> Result<reqwest::Response, StorageError> {
        let url = upload_url(&self.inner.bucket, path, if_generation_match);
        self.send(
            Method::POST,
            &url,
            Some(("text/plain; charset=utf-8".into(), bytes)),
        )
        .await
    }

    /// GET the object resource (generation / updated / metadata).
    async fn get_object(&self, path: &str) -> Result<Option<serde_json::Value>, StorageError> {
        let url = object_url(&self.inner.bucket, path);
        let response = self.send(Method::GET, &url, None).await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(ensure_success(response).await?.json().await?))
    }

    /// GET the object media, or `None` on 404. `if_generation_match` pins
    /// the read to a specific generation (versioned reads).
    async fn get_media(
        &self,
        path: &str,
        if_generation_match: Option<&str>,
    ) -> Result<Option<Vec<u8>>, StorageError> {
        let mut url = media_url(&self.inner.bucket, path);
        if let Some(generation) = if_generation_match {
            url.push_str(&format!("&ifGenerationMatch={generation}"));
        }
        let response = self.send(Method::GET, &url, None).await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(
            ensure_success(response).await?.bytes().await?.to_vec(),
        ))
    }
}

/// Return the response on success, otherwise lift the status + body into
/// [`StorageError::Gcs`].
async fn ensure_success(response: reqwest::Response) -> Result<reqwest::Response, StorageError> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();
    Err(StorageError::Gcs { status, body })
}

/// Parse an RFC3339 GCS timestamp ("2026-05-16T12:34:56.789Z").
fn parse_timestamp(value: &serde_json::Value) -> Option<DateTime<Utc>> {
    let raw = value.as_str()?;
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Percent-encode per RFC 3986: keep the unreserved set, encode everything
/// else as uppercase %XX of the UTF-8 bytes. Used both for the object path
/// segment of the JSON API (slash must become %2F) and for query params.
pub(crate) fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// POST endpoint for media upload; `name` travels as a query param.
pub(crate) fn upload_url(bucket: &str, name: &str, if_generation_match: Option<&str>) -> String {
    let mut url = format!(
        "{API_BASE}/upload/storage/v1/b/{bucket}/o?uploadType=media&name={}",
        percent_encode(name)
    );
    if let Some(generation) = if_generation_match {
        url.push_str(&format!("&ifGenerationMatch={generation}"));
    }
    url
}

/// Object resource endpoint; the object name is a single path segment, so
/// slashes inside the name must be encoded as %2F.
pub(crate) fn object_url(bucket: &str, name: &str) -> String {
    format!(
        "{API_BASE}/storage/v1/b/{bucket}/o/{}",
        percent_encode(name)
    )
}

/// Media download endpoint (`?alt=media`).
pub(crate) fn media_url(bucket: &str, name: &str) -> String {
    format!("{}?alt=media", object_url(bucket, name))
}

/// Object listing endpoint with `fields` projection and pagination.
pub(crate) fn list_url(
    bucket: &str,
    prefix: &str,
    page_token: Option<&str>,
    fields: &str,
) -> String {
    let mut url = format!(
        "{API_BASE}/storage/v1/b/{bucket}/o?prefix={}&fields={}",
        percent_encode(prefix),
        percent_encode(fields)
    );
    if let Some(token) = page_token {
        url.push_str(&format!("&pageToken={}", percent_encode(token)));
    }
    url
}

#[async_trait]
impl BlobBackend for GcsBackend {
    async fn upload_text(&self, path: &str, content: &str) -> Result<(), StorageError> {
        let response = self.upload(path, content.as_bytes().to_vec(), None).await?;
        ensure_success(response).await?;
        Ok(())
    }

    async fn upload_bytes(&self, path: &str, content: &[u8]) -> Result<(), StorageError> {
        let response = self.upload(path, content.to_vec(), None).await?;
        ensure_success(response).await?;
        Ok(())
    }

    async fn download_text(&self, path: &str) -> Result<Option<String>, StorageError> {
        let Some(bytes) = self.get_media(path, None).await? else {
            return Ok(None);
        };
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|err| StorageError::Other(format!("invalid UTF-8 in {path}: {err}")))
    }

    async fn download_bytes(&self, path: &str) -> Result<Option<Vec<u8>>, StorageError> {
        self.get_media(path, None).await
    }

    async fn download_to_filename(&self, path: &str, dest: &Path) -> Result<bool, StorageError> {
        let Some(bytes) = self.get_media(path, None).await? else {
            return Ok(false);
        };
        std::fs::write(dest, bytes)?;
        Ok(true)
    }

    async fn upload_text_if_absent(&self, path: &str, content: &str) -> Result<bool, StorageError> {
        let response = self
            .upload(path, content.as_bytes().to_vec(), Some("0"))
            .await?;
        if response.status() == StatusCode::PRECONDITION_FAILED {
            return Ok(false);
        }
        ensure_success(response).await?;
        Ok(true)
    }

    async fn upload_file_if_absent(
        &self,
        path: &str,
        local_file: &Path,
    ) -> Result<bool, StorageError> {
        let bytes = std::fs::read(local_file)?;
        let response = self.upload(path, bytes, Some("0")).await?;
        if response.status() == StatusCode::PRECONDITION_FAILED {
            return Ok(false);
        }
        ensure_success(response).await?;
        Ok(true)
    }

    async fn download_text_versioned(
        &self,
        path: &str,
    ) -> Result<Option<VersionedText>, StorageError> {
        // Python retries up to 3 times: between the generation read and the
        // pinned download another writer can bump the object (412).
        for attempt in 0..3 {
            let Some(object) = self.get_object(path).await? else {
                return Ok(None);
            };
            let generation = object
                .get("generation")
                .and_then(|g| g.as_str())
                .ok_or_else(|| StorageError::Other(format!("no generation for {path}")))?
                .to_string();
            match self.get_media(path, Some(&generation)).await {
                Ok(Some(bytes)) => {
                    let content = String::from_utf8(bytes).map_err(|err| {
                        StorageError::Other(format!("invalid UTF-8 in {path}: {err}"))
                    })?;
                    return Ok(Some(VersionedText {
                        content,
                        version: generation,
                    }));
                }
                Ok(None) => return Ok(None),
                Err(StorageError::Gcs { status: 412, .. }) if attempt < 2 => continue,
                Err(StorageError::Gcs { status: 412, .. }) => {
                    return Err(StorageError::StorageConflict(format!(
                        "{path} changed concurrently"
                    )));
                }
                Err(err) => return Err(err),
            }
        }
        Err(StorageError::Other("unreachable versioned GCS read".into()))
    }

    async fn compare_and_swap_text(
        &self,
        path: &str,
        expected_version: &str,
        content: &str,
    ) -> Result<String, StorageError> {
        let response = self
            .upload(path, content.as_bytes().to_vec(), Some(expected_version))
            .await?;
        // Python maps PreconditionFailed AND ResourceNotFoundError (a CAS
        // against a missing object) to StorageConflict.
        if matches!(
            response.status(),
            StatusCode::PRECONDITION_FAILED | StatusCode::NOT_FOUND
        ) {
            return Err(StorageError::StorageConflict(format!(
                "{path} changed concurrently"
            )));
        }
        // The upload response carries the object resource; its generation is
        // the version we just created. Do not re-read: a later writer could
        // otherwise make us return its generation.
        let object: serde_json::Value = ensure_success(response).await?.json().await?;
        object
            .get("generation")
            .and_then(|g| g.as_str())
            .map(str::to_string)
            .ok_or_else(|| StorageError::Other(format!("no generation in CAS response for {path}")))
    }

    async fn delete(&self, path: &str) -> Result<(), StorageError> {
        let url = object_url(&self.inner.bucket, path);
        let response = self.send(Method::DELETE, &url, None).await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        ensure_success(response).await?;
        Ok(())
    }

    async fn exists(&self, path: &str) -> Result<bool, StorageError> {
        Ok(self.get_object(path).await?.is_some())
    }

    async fn list_paths(
        &self,
        prefix: &str,
        oldest_first: usize,
    ) -> Result<Vec<String>, StorageError> {
        let fields = "items(name,timeCreated),nextPageToken";
        let mut items: Vec<(String, String)> = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let url = list_url(&self.inner.bucket, prefix, page_token.as_deref(), fields);
            let response = self.send(Method::GET, &url, None).await?;
            let page: serde_json::Value = ensure_success(response).await?.json().await?;
            if let Some(array) = page.get("items").and_then(|i| i.as_array()) {
                for item in array {
                    let name = item
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or_default();
                    let created = item
                        .get("timeCreated")
                        .and_then(|t| t.as_str())
                        .unwrap_or_default();
                    items.push((name.to_string(), created.to_string()));
                }
            }
            match page.get("nextPageToken").and_then(|t| t.as_str()) {
                Some(token) => page_token = Some(token.to_string()),
                None => break,
            }
        }
        if oldest_first > 0 {
            items.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
            items.truncate(oldest_first);
        }
        Ok(items.into_iter().map(|(name, _)| name).collect())
    }

    async fn updated_at(&self, path: &str) -> Result<Option<DateTime<Utc>>, StorageError> {
        let Some(object) = self.get_object(path).await? else {
            return Ok(None);
        };
        Ok(object.get("updated").and_then(parse_timestamp))
    }

    async fn set_metadata(
        &self,
        path: &str,
        kv: &BTreeMap<String, String>,
    ) -> Result<(), StorageError> {
        // Python: blob.reload(); blob.metadata = {**old, **new}; blob.patch().
        // A missing blob is a no-op here (LocalBackend semantics); the Python
        // GCS path would raise NotFound, but write_job always uploads first.
        let Some(object) = self.get_object(path).await? else {
            return Ok(());
        };
        let mut merged: BTreeMap<String, String> = object
            .get("metadata")
            .and_then(|m| m.as_object())
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        merged.extend(kv.iter().map(|(k, v)| (k.clone(), v.clone())));
        let body = serde_json::json!({ "metadata": merged });
        let url = object_url(&self.inner.bucket, path);
        let response = self
            .send(
                Method::PATCH,
                &url,
                Some(("application/json".into(), body.to_string().into_bytes())),
            )
            .await?;
        ensure_success(response).await?;
        Ok(())
    }

    async fn list_blobs_with_meta(&self, prefix: &str) -> Result<Vec<BlobInfo>, StorageError> {
        let fields = "items(name,updated,size,metadata),nextPageToken";
        let mut out = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let url = list_url(&self.inner.bucket, prefix, page_token.as_deref(), fields);
            let response = self.send(Method::GET, &url, None).await?;
            let page: serde_json::Value = ensure_success(response).await?.json().await?;
            if let Some(array) = page.get("items").and_then(|i| i.as_array()) {
                for item in array {
                    let metadata: BTreeMap<String, String> = item
                        .get("metadata")
                        .and_then(|m| m.as_object())
                        .map(|m| {
                            m.iter()
                                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                                .collect()
                        })
                        .unwrap_or_default();
                    out.push(BlobInfo {
                        name: item
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or_default()
                            .to_string(),
                        updated: item.get("updated").and_then(parse_timestamp),
                        size: item
                            .get("size")
                            .and_then(serde_json::Value::as_str)
                            .and_then(|value| value.parse::<u64>().ok()),
                        metadata,
                    });
                }
            }
            match page.get("nextPageToken").and_then(|t| t.as_str()) {
                Some(token) => page_token = Some(token.to_string()),
                None => break,
            }
        }
        Ok(out)
    }
}

