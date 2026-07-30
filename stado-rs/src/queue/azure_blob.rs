//! Azure Blob Storage backend using the provider REST API.
//!
//! Authentication uses managed identity first and then the scoped
//! `stado-azure` service-principal item in Skarbiec. Conditional creates and
//! writes use `If-None-Match` and `If-Match`; lost races surface as
//! [`StorageError::StorageConflict`]. Reads pin the observed ETag and retry a
//! bounded concurrent-write race. Listing preserves opaque continuation
//! markers and blob metadata.
//!
//! The REST API version is pinned so conditional headers, metadata, and
//! pagination remain a release-visible contract.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::{Method, StatusCode};

use super::{gcs::percent_encode, BlobBackend, BlobInfo, StorageError, VersionedText};

/// Pinned Blob Storage REST API version (see module docs). Shared with
/// the release-channel fetcher in [`crate::self_update`], which speaks the
/// same REST surface when the release tree lives in a blob container.
pub(crate) const X_MS_VERSION: &str = "2023-11-03";
/// OAuth scope for the client-credentials token request.
pub(crate) const STORAGE_SCOPE: &str = "https://storage.azure.com/.default";
/// Resource for IMDS / az-CLI token requests (same audience).
pub(crate) const STORAGE_RESOURCE: &str = "https://storage.azure.com";

struct Inner {
    http: reqwest::Client,
    account: String,
    container: String,
    /// `https://{account}.blob.core.windows.net` in prod, loopback in tests.
    base_url: String,
    /// True in prod (token chain attached); false on loopback test mocks.
    auth: bool,
}

impl std::fmt::Debug for Inner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inner")
            .field("account", &self.account)
            .field("container", &self.container)
            .field("base_url", &self.base_url)
            .field("auth", &self.auth)
            .finish_non_exhaustive()
    }
}

/// Azure Blob implementation of [`BlobBackend`]. Cheap to clone.
#[derive(Clone, Debug)]
pub struct AzureBlobBackend {
    inner: Arc<Inner>,
}

impl AzureBlobBackend {
    /// Build a backend for `account`/`container`. Empty values are the
    /// Python RuntimeErrors, verbatim.
    pub fn new(account: &str, container: &str) -> Result<Self, StorageError> {
        if account.is_empty() {
            return Err(StorageError::Other(
                "WC_AZURE_STORAGE_ACCOUNT env var is empty; cannot construct AzureBlobBackend"
                    .into(),
            ));
        }
        if container.is_empty() {
            return Err(StorageError::Other(
                "WC_AZURE_CONTAINER env var is empty; cannot construct AzureBlobBackend".into(),
            ));
        }
        Ok(Self::assemble(
            &format!("https://{account}.blob.core.windows.net"),
            account,
            container,
            true,
        ))
    }

    /// Bind to an explicit base URL without auth (loopback mocks in tests).
    #[cfg(test)]
    pub(crate) fn for_test(base_url: &str, container: &str) -> Self {
        Self::assemble(base_url, "testaccount", container, false)
    }

    fn assemble(base_url: &str, account: &str, container: &str, auth: bool) -> Self {
        Self {
            inner: Arc::new(Inner {
                http: reqwest::Client::new(),
                account: account.to_string(),
                container: container.to_string(),
                base_url: base_url.trim_end_matches('/').to_string(),
                auth,
            }),
        }
    }

    /// The storage account (Python `self.account`).
    pub fn account(&self) -> &str {
        &self.inner.account
    }

    /// The container (bucket-equivalent; Python `self.container_name`).
    pub fn container(&self) -> &str {
        &self.inner.container
    }

    /// `/{container}/{path}` with the blob name percent-encoded per segment
    /// (slash separators preserved).
    fn blob_url(&self, path: &str) -> String {
        let encoded = path
            .split('/')
            .map(percent_encode)
            .collect::<Vec<_>>()
            .join("/");
        format!("{}/{}/{encoded}", self.inner.base_url, self.inner.container)
    }

    /// Container List Blobs URL; `marker` continues a paginated listing,
    /// `include_metadata` adds each blob's metadata to the response
    /// (Python `include=["metadata"]`).
    fn list_url(&self, prefix: &str, marker: Option<&str>, include_metadata: bool) -> String {
        let mut url = format!(
            "{}/{}?restype=container&comp=list&prefix={}",
            self.inner.base_url,
            self.inner.container,
            percent_encode(prefix)
        );
        if include_metadata {
            url.push_str("&include=metadata");
        }
        if let Some(marker) = marker {
            url.push_str(&format!("&marker={}", percent_encode(marker)));
        }
        url
    }

    /// Send one authenticated request with the pinned API version. Extra
    /// headers are caller-supplied (conditionals, blob type, metadata).
    async fn send(
        &self,
        method: Method,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<Vec<u8>>,
    ) -> Result<reqwest::Response, StorageError> {
        let mut request = self
            .inner
            .http
            .request(method, url)
            .header("x-ms-version", X_MS_VERSION)
            // RFC1123 IMF-fixdate, e.g. "Sun, 26 Jul 2026 03:44:52 GMT".
            .header(
                "x-ms-date",
                Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string(),
            );
        if self.inner.auth {
            let token =
                crate::azure_token::bearer_token(&self.inner.http, STORAGE_SCOPE, STORAGE_RESOURCE)
                    .await
                    .map_err(|err| match err {
                        crate::azure_token::TokenError::Auth(msg) => StorageError::Auth(msg),
                        crate::azure_token::TokenError::Http(err) => StorageError::Http(err),
                    })?;
            request = request.header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"));
        }
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        if let Some(body) = body {
            request = request.body(body);
        }
        Ok(request.send().await?)
    }

    /// Lift a non-success response into [`StorageError::Other`], truncating
    /// the body like the provider's ARM error surface.
    async fn api_error(response: reqwest::Response, op: &str) -> StorageError {
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        StorageError::Other(format!(
            "Azure blob {op} -> HTTP {status}: {}",
            text.chars().take(280).collect::<String>()
        ))
    }

    /// Pass through success; anything else becomes an error via
    /// [`Self::api_error`].
    async fn ensure_success(
        response: reqwest::Response,
        op: &str,
    ) -> Result<reqwest::Response, StorageError> {
        if response.status().is_success() {
            return Ok(response);
        }
        Err(Self::api_error(response, op).await)
    }

    /// GET the blob body, or `None` on 404 (Python
    /// `ResourceNotFoundError`).
    async fn get_bytes(&self, path: &str, if_match: Option<&str>) -> GetOutcome {
        let headers: Vec<(&str, &str)> = match if_match {
            Some(etag) => vec![("If-Match", etag)],
            None => Vec::new(),
        };
        let response = match self
            .send(Method::GET, &self.blob_url(path), &headers, None)
            .await
        {
            Ok(response) => response,
            Err(err) => return GetOutcome::Error(err),
        };
        match response.status() {
            StatusCode::NOT_FOUND => GetOutcome::NotFound,
            StatusCode::PRECONDITION_FAILED => GetOutcome::PreconditionFailed,
            status if status.is_success() => match response.bytes().await {
                Ok(bytes) => GetOutcome::Ok(bytes.to_vec()),
                Err(err) => GetOutcome::Error(err.into()),
            },
            _ => GetOutcome::Error(Self::api_error(response, &format!("GET {path}")).await),
        }
    }

    /// HEAD the blob (Get Blob Properties): (etag, last_modified, metadata)
    /// or `None` on 404.
    async fn head(&self, path: &str) -> Result<Option<BlobProps>, StorageError> {
        let response = self
            .send(Method::HEAD, &self.blob_url(path), &[], None)
            .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response = Self::ensure_success(response, &format!("HEAD {path}")).await?;
        let etag = header_str(&response, "etag");
        let last_modified = header_str(&response, "last-modified")
            .as_deref()
            .and_then(parse_http_date);
        let metadata = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                let key = name.as_str().strip_prefix("x-ms-meta-")?;
                Some((key.to_string(), value.to_str().ok()?.to_string()))
            })
            .collect();
        Ok(Some(BlobProps {
            etag,
            last_modified,
            metadata,
        }))
    }

    /// Put Blob; `conditional` is `If-None-Match: *` (if-absent) or
    /// `If-Match: etag` (CAS). Returns the raw response for status mapping.
    async fn put_blob(
        &self,
        path: &str,
        bytes: Vec<u8>,
        conditional: Option<(&str, &str)>,
    ) -> Result<reqwest::Response, StorageError> {
        let mut headers: Vec<(&str, &str)> = vec![("x-ms-blob-type", "BlockBlob")];
        if let Some((name, value)) = conditional {
            headers.push((name, value));
        }
        self.send(Method::PUT, &self.blob_url(path), &headers, Some(bytes))
            .await
    }

    /// PUT with `If-None-Match: *`; `false` on 409 (Python
    /// `ResourceExistsError`).
    async fn upload_bytes_if_absent(
        &self,
        path: &str,
        bytes: Vec<u8>,
    ) -> Result<bool, StorageError> {
        let response = self
            .put_blob(path, bytes, Some(("If-None-Match", "*")))
            .await?;
        if response.status() == StatusCode::CONFLICT {
            return Ok(false);
        }
        Self::ensure_success(response, &format!("PUT {path} if-absent")).await?;
        Ok(true)
    }

    /// One paginated List Blobs walk, parsed into raw entries.
    async fn list_entries(
        &self,
        prefix: &str,
        include_metadata: bool,
    ) -> Result<Vec<ListEntry>, StorageError> {
        let mut out = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let url = self.list_url(prefix, marker.as_deref(), include_metadata);
            let response = self.send(Method::GET, &url, &[], None).await?;
            let body = Self::ensure_success(response, &format!("list {prefix}"))
                .await?
                .text()
                .await?;
            let (entries, next) = parse_list_blobs(&body);
            out.extend(entries);
            match next {
                Some(next) if !next.is_empty() => marker = Some(next),
                _ => break,
            }
        }
        Ok(out)
    }
}

/// Outcome of a pinned/unpinned GET, so the versioned-read retry loop can
/// distinguish the three Python exception branches.
enum GetOutcome {
    Ok(Vec<u8>),
    NotFound,
    PreconditionFailed,
    Error(StorageError),
}

/// Result of Get Blob Properties.
struct BlobProps {
    etag: Option<String>,
    last_modified: Option<DateTime<Utc>>,
    metadata: BTreeMap<String, String>,
}

/// One `<Blob>` entry of a List Blobs response.
struct ListEntry {
    name: String,
    creation_time: Option<DateTime<Utc>>,
    last_modified: Option<DateTime<Utc>>,
    size: Option<u64>,
    metadata: BTreeMap<String, String>,
}

/// Read a response header as an owned string.
fn header_str(response: &reqwest::Response, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)?
        .to_str()
        .ok()
        .map(str::to_string)
}

/// Parse an RFC1123 HTTP date ("Fri, 02 Jan 2026 03:04:05 GMT") — the
/// format of Last-Modified / Creation-Time in headers and list XML.
fn parse_http_date(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc2822(raw)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Text content of the first `<tag>...</tag>` in `xml` (plain tags only —
/// the List Blobs payload carries no attributes inside `<Blobs>`).
fn xml_tag<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(&xml[start..end])
}

/// Decode the five predefined XML entities (`&amp;` last so an escaped
/// ampersand in front of another entity's name stays literal).
fn xml_unescape(raw: &str) -> String {
    raw.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Parse the `<Metadata>` children of one blob block.
fn parse_metadata(block: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(mut rest) = xml_tag(block, "Metadata") else {
        return out;
    };
    while let Some(open_end) = rest.find('>') {
        let tag = &rest[1..open_end];
        if tag.is_empty() || tag.contains(['<', '/', ' ']) {
            break;
        }
        let close = format!("</{tag}>");
        let Some(close_start) = rest.find(&close) else {
            break;
        };
        let value = &rest[open_end + 1..close_start];
        out.insert(tag.to_string(), xml_unescape(value));
        rest = &rest[close_start + close.len()..];
        if !rest.starts_with('<') {
            break;
        }
    }
    out
}

/// Parse a List Blobs response: the `<Blob>` entries plus the
/// `<NextMarker>` continuation (empty/absent = final page).
fn parse_list_blobs(xml: &str) -> (Vec<ListEntry>, Option<String>) {
    let mut entries = Vec::new();
    let mut rest = xml;
    while let Some(blob_start) = rest.find("<Blob>") {
        let after_open = &rest[blob_start + "<Blob>".len()..];
        let Some(blob_end) = after_open.find("</Blob>") else {
            break;
        };
        let block = &after_open[..blob_end];
        entries.push(ListEntry {
            name: xml_tag(block, "Name").map(xml_unescape).unwrap_or_default(),
            creation_time: xml_tag(block, "Creation-Time").and_then(parse_http_date),
            last_modified: xml_tag(block, "Last-Modified").and_then(parse_http_date),
            size: xml_tag(block, "Content-Length").and_then(|value| value.parse().ok()),
            metadata: parse_metadata(block),
        });
        rest = &after_open[blob_end + "</Blob>".len()..];
    }
    let next = xml_tag(rest, "NextMarker").map(xml_unescape);
    (entries, next)
}

#[async_trait]
impl BlobBackend for AzureBlobBackend {
    async fn upload_text(&self, path: &str, content: &str) -> Result<(), StorageError> {
        self.upload_bytes(path, content.as_bytes()).await
    }

    async fn upload_bytes(&self, path: &str, content: &[u8]) -> Result<(), StorageError> {
        let response = self.put_blob(path, content.to_vec(), None).await?;
        Self::ensure_success(response, &format!("PUT {path}")).await?;
        Ok(())
    }

    async fn download_text(&self, path: &str) -> Result<Option<String>, StorageError> {
        let Some(bytes) = self.download_bytes(path).await? else {
            return Ok(None);
        };
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|err| StorageError::Other(format!("invalid UTF-8 in {path}: {err}")))
    }

    async fn download_bytes(&self, path: &str) -> Result<Option<Vec<u8>>, StorageError> {
        match self.get_bytes(path, None).await {
            GetOutcome::Ok(bytes) => Ok(Some(bytes)),
            GetOutcome::NotFound => Ok(None),
            GetOutcome::PreconditionFailed => unreachable!("unpinned GET cannot 412"),
            GetOutcome::Error(err) => Err(err),
        }
    }

    async fn download_to_filename(&self, path: &str, dest: &Path) -> Result<bool, StorageError> {
        let Some(bytes) = self.download_bytes(path).await? else {
            return Ok(false);
        };
        std::fs::write(dest, bytes)?;
        Ok(true)
    }

    async fn upload_text_if_absent(&self, path: &str, content: &str) -> Result<bool, StorageError> {
        self.upload_bytes_if_absent(path, content.as_bytes().to_vec())
            .await
    }

    async fn upload_file_if_absent(
        &self,
        path: &str,
        local_file: &Path,
    ) -> Result<bool, StorageError> {
        let bytes = std::fs::read(local_file)?;
        self.upload_bytes_if_absent(path, bytes).await
    }

    async fn download_text_versioned(
        &self,
        path: &str,
    ) -> Result<Option<VersionedText>, StorageError> {
        // Python: 3 attempts of (properties -> pinned download); a 412
        // between HEAD and GET retries, the final 412 re-raises.
        for attempt in 0..3 {
            let Some(props) = self.head(path).await? else {
                return Ok(None);
            };
            let etag = props.etag.unwrap_or_default();
            match self.get_bytes(path, Some(&etag)).await {
                GetOutcome::Ok(bytes) => {
                    let content = String::from_utf8(bytes).map_err(|err| {
                        StorageError::Other(format!("invalid UTF-8 in {path}: {err}"))
                    })?;
                    // Python returns str(props.etag) — the HEAD ETag,
                    // verbatim (quoted).
                    return Ok(Some(VersionedText {
                        content,
                        version: etag,
                    }));
                }
                GetOutcome::NotFound => return Ok(None),
                GetOutcome::PreconditionFailed if attempt < 2 => continue,
                GetOutcome::PreconditionFailed => {
                    // Python re-raises ResourceModifiedError here (NOT a
                    // StorageConflict).
                    return Err(StorageError::Other(format!(
                        "azure blob {path} changed during versioned read (3 attempts)"
                    )));
                }
                GetOutcome::Error(err) => return Err(err),
            }
        }
        Err(StorageError::Other(
            "unreachable versioned Azure Blob read".into(),
        ))
    }

    async fn compare_and_swap_text(
        &self,
        path: &str,
        expected_version: &str,
        content: &str,
    ) -> Result<String, StorageError> {
        let response = self
            .put_blob(
                path,
                content.as_bytes().to_vec(),
                Some(("If-Match", expected_version)),
            )
            .await?;
        if response.status() == StatusCode::PRECONDITION_FAILED {
            return Err(StorageError::StorageConflict(format!(
                "{path} changed concurrently"
            )));
        }
        let response = Self::ensure_success(response, &format!("conditional PUT {path}")).await?;
        // Python: response.get("etag") or RuntimeError.
        header_str(&response, "etag").ok_or_else(|| {
            StorageError::Other("Azure conditional write did not return its ETag".into())
        })
    }

    async fn delete(&self, path: &str) -> Result<(), StorageError> {
        let response = self
            .send(Method::DELETE, &self.blob_url(path), &[], None)
            .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(());
        }
        Self::ensure_success(response, &format!("DELETE {path}")).await?;
        Ok(())
    }

    async fn exists(&self, path: &str) -> Result<bool, StorageError> {
        let response = self
            .send(Method::HEAD, &self.blob_url(path), &[], None)
            .await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(false);
        }
        Self::ensure_success(response, &format!("HEAD {path}")).await?;
        Ok(true)
    }

    async fn list_paths(
        &self,
        prefix: &str,
        oldest_first: usize,
    ) -> Result<Vec<String>, StorageError> {
        let mut entries = self.list_entries(prefix, false).await?;
        if oldest_first > 0 {
            // Python: sort by creation_time ascending (None sorts as
            // datetime.min), take the N oldest.
            entries.sort_by_key(|entry| entry.creation_time);
            entries.truncate(oldest_first);
        }
        Ok(entries.into_iter().map(|entry| entry.name).collect())
    }

    async fn updated_at(&self, path: &str) -> Result<Option<DateTime<Utc>>, StorageError> {
        Ok(self.head(path).await?.and_then(|props| props.last_modified))
    }

    async fn set_metadata(
        &self,
        path: &str,
        kv: &BTreeMap<String, String>,
    ) -> Result<(), StorageError> {
        let Some(props) = self.head(path).await? else {
            return Err(StorageError::NotFound(path.to_string()));
        };
        let mut merged: BTreeMap<String, String> = props.metadata;
        merged.extend(
            kv.iter()
                .filter(|(_, v)| !v.is_empty())
                .map(|(k, v)| (k.clone(), v.clone())),
        );
        let headers: Vec<(String, String)> = merged
            .iter()
            .map(|(k, v)| (format!("x-ms-meta-{k}"), v.clone()))
            .collect();
        let header_refs: Vec<(&str, &str)> = headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let url = format!("{}?comp=metadata", self.blob_url(path));
        let response = self.send(Method::PUT, &url, &header_refs, None).await?;
        Self::ensure_success(response, &format!("set_metadata {path}")).await?;
        Ok(())
    }

    async fn list_blobs_with_meta(&self, prefix: &str) -> Result<Vec<BlobInfo>, StorageError> {
        Ok(self
            .list_entries(prefix, true)
            .await?
            .into_iter()
            .map(|entry| BlobInfo {
                name: entry.name,
                updated: entry.last_modified,
                size: entry.size,
                metadata: entry.metadata,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::mock_http;

    /// HTTP response with custom headers (ETag, Last-Modified, x-ms-meta-*).
    fn response_with(status: u16, reason: &str, headers: &[(&str, &str)], body: &str) -> String {
        let extra: String = headers
            .iter()
            .map(|(k, v)| format!("{k}: {v}\r\n"))
            .collect();
        format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/xml\r\n\
             Content-Length: {}\r\nConnection: close\r\n{extra}\r\n{body}",
            body.len()
        )
    }

    fn requests(server: &crate::testutil::MockHttp) -> Vec<String> {
        server.requests.lock().unwrap().clone()
    }

    fn backend(server: &crate::testutil::MockHttp) -> AzureBlobBackend {
        AzureBlobBackend::for_test(&server.base_url, "cont")
    }

    fn list_xml(entries: &str, next_marker: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
             <EnumerationResults ServiceEndpoint=\"https://acct.blob.core.windows.net/\" ContainerName=\"cont\">\
             <Prefix>queue/</Prefix><MaxResults>5000</MaxResults>\
             <Blobs>{entries}</Blobs>\
             <NextMarker>{next_marker}</NextMarker></EnumerationResults>"
        )
    }

    #[tokio::test]
    async fn upload_and_download_round_trip() {
        let server = mock_http(vec![
            response_with(201, "Created", &[("ETag", "\"0x8D1\"")], ""),
            response_with(200, "OK", &[("ETag", "\"0x8D1\"")], "hello"),
            response_with(404, "Not Found", &[], ""),
        ])
        .await;
        let b = backend(&server);
        b.upload_text("queue/j1.json", "hello").await.unwrap();
        assert_eq!(
            b.download_text("queue/j1.json").await.unwrap().as_deref(),
            Some("hello")
        );
        assert_eq!(b.download_text("gone").await.unwrap(), None);
        let reqs = requests(&server);
        assert_eq!(reqs.len(), 3, "{reqs:?}");
        assert!(
            reqs[0].starts_with("PUT /cont/queue/j1.json"),
            "{}",
            reqs[0]
        );
        assert!(reqs[0].contains("x-ms-blob-type: BlockBlob"), "{}", reqs[0]);
        assert!(
            reqs[0].contains(&format!("x-ms-version: {X_MS_VERSION}")),
            "{}",
            reqs[0]
        );
        assert!(reqs[0].contains("x-ms-date: "), "{}", reqs[0]);
        assert!(reqs[0].contains("hello"), "{}", reqs[0]);
        assert!(
            reqs[1].starts_with("GET /cont/queue/j1.json"),
            "{}",
            reqs[1]
        );
        server.stop();
    }

    #[tokio::test]
    async fn download_to_filename_reports_absence() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");
        let server = mock_http(vec![
            response_with(404, "Not Found", &[], ""),
            response_with(200, "OK", &[], "data"),
        ])
        .await;
        let b = backend(&server);
        assert!(!b.download_to_filename("nope", &dest).await.unwrap());
        assert!(b.download_to_filename("blob", &dest).await.unwrap());
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "data");
        server.stop();
    }

    #[tokio::test]
    async fn upload_if_absent_maps_409_to_false() {
        let server = mock_http(vec![
            response_with(201, "Created", &[("ETag", "\"0x8D1\"")], ""),
            response_with(
                409,
                "Conflict",
                &[],
                "<Error><Code>BlobAlreadyExists</Code></Error>",
            ),
        ])
        .await;
        let b = backend(&server);
        assert!(b.upload_text_if_absent("lock", "first").await.unwrap());
        assert!(!b.upload_text_if_absent("lock", "second").await.unwrap());
        let reqs = requests(&server);
        assert!(reqs[0].contains("if-none-match: *"), "{}", reqs[0]);
        assert!(reqs[1].contains("if-none-match: *"), "{}", reqs[1]);
        server.stop();
    }

    #[tokio::test]
    async fn upload_file_if_absent_uploads_once() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.txt");
        std::fs::write(&src, "file-bytes").unwrap();
        let server = mock_http(vec![
            response_with(201, "Created", &[("ETag", "\"0x8D1\"")], ""),
            response_with(409, "Conflict", &[], ""),
        ])
        .await;
        let b = backend(&server);
        assert!(b.upload_file_if_absent("f", &src).await.unwrap());
        assert!(!b.upload_file_if_absent("f", &src).await.unwrap());
        server.stop();
    }

    #[tokio::test]
    async fn versioned_read_pins_get_to_head_etag() {
        let server = mock_http(vec![
            // HEAD properties.
            response_with(200, "OK", &[("ETag", "\"0x8DAAA\"")], ""),
            // Pinned GET.
            response_with(200, "OK", &[("ETag", "\"0x8DAAA\"")], "payload"),
            // Missing blob: HEAD 404 -> None.
            response_with(404, "Not Found", &[], ""),
        ])
        .await;
        let b = backend(&server);
        let v = b.download_text_versioned("state/x").await.unwrap().unwrap();
        assert_eq!(v.content, "payload");
        // Python parity: version token is str(props.etag) — QUOTES KEPT.
        assert_eq!(v.version, "\"0x8DAAA\"");
        assert_eq!(b.download_text_versioned("gone").await.unwrap(), None);
        let reqs = requests(&server);
        assert!(reqs[0].starts_with("HEAD /cont/state/x"), "{}", reqs[0]);
        assert!(reqs[1].starts_with("GET /cont/state/x"), "{}", reqs[1]);
        // MatchConditions.IfNotModified -> If-Match header with the HEAD etag.
        assert!(reqs[1].contains("if-match: \"0x8DAAA\""), "{}", reqs[1]);
        server.stop();
    }

    #[tokio::test]
    async fn versioned_read_retries_412_then_succeeds() {
        let server = mock_http(vec![
            // Attempt 1: HEAD ok, GET 412 (blob changed between the two).
            response_with(200, "OK", &[("ETag", "\"0x8D1\"")], ""),
            response_with(412, "Precondition Failed", &[], ""),
            // Attempt 2: HEAD ok, GET ok.
            response_with(200, "OK", &[("ETag", "\"0x8D2\"")], ""),
            response_with(200, "OK", &[("ETag", "\"0x8D2\"")], "v2"),
        ])
        .await;
        let b = backend(&server);
        let v = b.download_text_versioned("state/x").await.unwrap().unwrap();
        assert_eq!(v.content, "v2");
        assert_eq!(v.version, "\"0x8D2\"");
        assert_eq!(requests(&server).len(), 4);
        server.stop();
    }

    #[tokio::test]
    async fn versioned_read_final_412_is_not_storage_conflict() {
        // 3 attempts x (HEAD + 412 GET) = 6 requests; the error is a plain
        // error like the Python ResourceModifiedError, NOT StorageConflict.
        let server = mock_http(vec![
            response_with(200, "OK", &[("ETag", "\"0x8D1\"")], ""),
            response_with(412, "Precondition Failed", &[], ""),
            response_with(200, "OK", &[("ETag", "\"0x8D2\"")], ""),
            response_with(412, "Precondition Failed", &[], ""),
            response_with(200, "OK", &[("ETag", "\"0x8D3\"")], ""),
            response_with(412, "Precondition Failed", &[], ""),
        ])
        .await;
        let b = backend(&server);
        let err = b.download_text_versioned("state/x").await.unwrap_err();
        assert!(!matches!(err, StorageError::StorageConflict(_)), "{err:?}");
        assert!(
            err.to_string().contains("changed during versioned read"),
            "{err}"
        );
        server.stop();
    }

    #[tokio::test]
    async fn cas_success_returns_response_etag() {
        let server = mock_http(vec![response_with(
            201,
            "Created",
            &[("ETag", "\"0x8D2\"")],
            "",
        )])
        .await;
        let b = backend(&server);
        let new_version = b
            .compare_and_swap_text("state/x", "\"0x8D1\"", "new")
            .await
            .unwrap();
        // Token round-trips verbatim (quoted), matching Python.
        assert_eq!(new_version, "\"0x8D2\"");
        let reqs = requests(&server);
        assert!(reqs[0].contains("if-match: \"0x8D1\""), "{}", reqs[0]);
        server.stop();
    }

    #[tokio::test]
    async fn cas_conflict_maps_412_to_storage_conflict() {
        let server = mock_http(vec![response_with(412, "Precondition Failed", &[], "")]).await;
        let b = backend(&server);
        let err = b
            .compare_and_swap_text("state/x", "\"0x8D1\"", "new")
            .await
            .unwrap_err();
        assert!(matches!(err, StorageError::StorageConflict(_)), "{err:?}");
        assert!(err.to_string().contains("changed concurrently"), "{err}");
        server.stop();
    }

    #[tokio::test]
    async fn delete_is_idempotent_and_exists_propagates_provider_failures() {
        let server = mock_http(vec![
            response_with(202, "Accepted", &[], ""),
            response_with(404, "Not Found", &[], ""),
            response_with(200, "OK", &[("ETag", "\"0x8D1\"")], ""),
            response_with(404, "Not Found", &[], ""),
            // exists on a non-404 failure: Python `except Exception` -> False.
            response_with(500, "Server Error", &[], ""),
        ])
        .await;
        let b = backend(&server);
        b.delete("blob").await.unwrap();
        b.delete("gone").await.unwrap(); // 404 tolerated
        assert!(b.exists("present").await.unwrap());
        assert!(!b.exists("missing").await.unwrap());
        let error = b.exists("server-on-fire").await.unwrap_err();
        assert!(
            error.to_string().contains(&format!(
                "HTTP {}",
                StatusCode::INTERNAL_SERVER_ERROR.as_u16()
            )),
            "{error}"
        );
        let reqs = requests(&server);
        assert!(reqs[0].starts_with("DELETE /cont/blob"), "{}", reqs[0]);
        assert!(reqs[2].starts_with("HEAD /cont/present"), "{}", reqs[2]);
        server.stop();
    }

    #[tokio::test]
    async fn updated_at_parses_last_modified_and_propagates_errors() {
        let server = mock_http(vec![
            response_with(
                200,
                "OK",
                &[
                    ("ETag", "\"0x8D1\""),
                    ("Last-Modified", "Fri, 02 Jan 2026 03:04:05 GMT"),
                ],
                "",
            ),
            response_with(404, "Not Found", &[], ""),
            response_with(500, "Server Error", &[], ""),
        ])
        .await;
        let b = backend(&server);
        assert_eq!(
            b.updated_at("m").await.unwrap().unwrap().to_rfc3339(),
            "2026-01-02T03:04:05+00:00"
        );
        assert_eq!(b.updated_at("gone").await.unwrap(), None);
        let error = b.updated_at("server-on-fire").await.unwrap_err();
        assert!(
            error.to_string().contains(&format!(
                "HTTP {}",
                StatusCode::INTERNAL_SERVER_ERROR.as_u16()
            )),
            "{error}"
        );
        server.stop();
    }

    #[tokio::test]
    async fn list_paths_paginates_and_bounded_sorts_by_creation_time() {
        let page1 = list_xml(
            "<Blob><Name>queue/b.json</Name><Properties>\
             <Creation-Time>Fri, 02 Jan 2026 03:04:05 GMT</Creation-Time>\
             <Last-Modified>Fri, 02 Jan 2026 03:04:05 GMT</Last-Modified>\
             </Properties></Blob>\
             <Blob><Name>queue/a.json</Name><Properties>\
             <Creation-Time>Sat, 03 Jan 2026 03:04:05 GMT</Creation-Time>\
             <Last-Modified>Sat, 03 Jan 2026 03:04:05 GMT</Last-Modified>\
             </Properties></Blob>",
            "marker-2/x",
        );
        let page2 = list_xml(
            "<Blob><Name>queue/c.json</Name><Properties>\
             <Creation-Time>Thu, 01 Jan 2026 03:04:05 GMT</Creation-Time>\
             <Last-Modified>Thu, 01 Jan 2026 03:04:05 GMT</Last-Modified>\
             </Properties></Blob>",
            "",
        );
        let server = mock_http(vec![
            response_with(200, "OK", &[], &page1),
            response_with(200, "OK", &[], &page2),
            response_with(200, "OK", &[], &page1),
            response_with(200, "OK", &[], &page2),
        ])
        .await;
        let b = backend(&server);
        // Unbounded: listing order preserved (Python parity — no sort).
        assert_eq!(
            b.list_paths("queue/", 0).await.unwrap(),
            vec!["queue/b.json", "queue/a.json", "queue/c.json"]
        );
        // Bounded: N oldest by Creation-Time.
        assert_eq!(
            b.list_paths("queue/", 2).await.unwrap(),
            vec!["queue/c.json", "queue/b.json"]
        );
        let reqs = requests(&server);
        assert!(
            reqs[0].starts_with("GET /cont?restype=container&comp=list&prefix=queue%2F"),
            "{}",
            reqs[0]
        );
        assert!(reqs[1].contains("marker=marker-2%2Fx"), "{}", reqs[1]);
        server.stop();
    }

    #[tokio::test]
    async fn set_metadata_merges_and_puts_comp_metadata() {
        let server = mock_http(vec![
            // HEAD: current metadata a=1.
            response_with(200, "OK", &[("x-ms-meta-a", "1")], ""),
            // PUT ?comp=metadata.
            response_with(200, "OK", &[("ETag", "\"0x8D2\"")], ""),
        ])
        .await;
        let b = backend(&server);
        b.set_metadata(
            "queue/j1.json",
            &BTreeMap::from([
                ("b".to_string(), "2".to_string()),
                ("empty".to_string(), String::new()),
            ]),
        )
        .await
        .unwrap();
        let reqs = requests(&server);
        assert!(
            reqs[1].starts_with("PUT /cont/queue/j1.json?comp=metadata"),
            "{}",
            reqs[1]
        );
        assert!(reqs[1].contains("x-ms-meta-a: 1"), "{}", reqs[1]);
        assert!(reqs[1].contains("x-ms-meta-b: 2"), "{}", reqs[1]);
        // Empty new values are skipped (Python parity).
        assert!(!reqs[1].contains("x-ms-meta-empty"), "{}", reqs[1]);
        server.stop();
    }

    #[tokio::test]
    async fn set_metadata_failures_propagate() {
        let missing = mock_http(vec![response_with(
            StatusCode::NOT_FOUND.as_u16(),
            "Not Found",
            &[],
            "",
        )])
        .await;
        let missing_backend = backend(&missing);
        let error = missing_backend
            .set_metadata("gone", &BTreeMap::from([("k".into(), "v".into())]))
            .await
            .unwrap_err();
        assert!(matches!(error, StorageError::NotFound(_)), "{error:?}");
        missing.stop();

        let provider_failure = mock_http(vec![
            response_with(StatusCode::OK.as_u16(), "OK", &[("ETag", "\"0x8D1\"")], ""),
            response_with(
                StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                "Server Error",
                &[],
                "boom",
            ),
        ])
        .await;
        let provider_backend = backend(&provider_failure);
        let error = provider_backend
            .set_metadata("present", &BTreeMap::from([("k".into(), "v".into())]))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains(&format!(
                "HTTP {}",
                StatusCode::INTERNAL_SERVER_ERROR.as_u16()
            )),
            "{error}"
        );
        provider_failure.stop();
    }

    #[tokio::test]
    async fn list_blobs_with_meta_includes_metadata() {
        let page = list_xml(
            "<Blob><Name>queue/j1.json</Name><Properties>\
             <Creation-Time>Thu, 01 Jan 2026 03:04:05 GMT</Creation-Time>\
             <Last-Modified>Fri, 02 Jan 2026 03:04:05 GMT</Last-Modified>\
             </Properties>\
             <Metadata><gpu_mem_gb>24</gpu_mem_gb><gpu_type>nvidia-l4</gpu_type>\
             <escaped>a&amp;lt;b</escaped></Metadata></Blob>\
             <Blob><Name>queue/j2.json</Name><Properties>\
             <Last-Modified>Sat, 03 Jan 2026 03:04:05 GMT</Last-Modified>\
             </Properties></Blob>",
            "",
        );
        let server = mock_http(vec![response_with(200, "OK", &[], &page)]).await;
        let b = backend(&server);
        let infos = b.list_blobs_with_meta("queue/").await.unwrap();
        assert_eq!(infos.len(), 2);
        assert_eq!(infos[0].name, "queue/j1.json");
        assert_eq!(
            infos[0].updated.unwrap().to_rfc3339(),
            "2026-01-02T03:04:05+00:00"
        );
        assert_eq!(
            infos[0].metadata,
            BTreeMap::from([
                ("gpu_mem_gb".to_string(), "24".to_string()),
                ("gpu_type".to_string(), "nvidia-l4".to_string()),
                // xml_unescape: &amp;lt; -> "&lt;" (literal, not "<").
                ("escaped".to_string(), "a&lt;b".to_string()),
            ])
        );
        // Blob without a Metadata block -> empty map.
        assert_eq!(infos[1].name, "queue/j2.json");
        assert!(infos[1].metadata.is_empty());
        let reqs = requests(&server);
        assert!(reqs[0].contains("include=metadata"), "{}", reqs[0]);
        server.stop();
    }

    #[test]
    fn xml_tag_extracts_first_plain_tag() {
        let xml = "<A><Name>x</Name><Name>y</Name></A>";
        assert_eq!(xml_tag(xml, "Name"), Some("x"));
        assert_eq!(xml_tag(xml, "Missing"), None);
        // Attribute-bearing outer tags are not matched as plain tags.
        assert_eq!(xml_tag("<A foo=\"1\">z</A>", "A"), None);
    }

    #[test]
    fn xml_unescape_order_keeps_escaped_ampersands_literal() {
        assert_eq!(xml_unescape("&lt;a&gt;"), "<a>");
        assert_eq!(xml_unescape("&quot;q&quot; &apos;a&apos;"), "\"q\" 'a'");
        assert_eq!(xml_unescape("a &amp; b"), "a & b");
        assert_eq!(xml_unescape("&amp;lt;"), "&lt;");
    }

    #[test]
    fn parse_http_date_reads_rfc1123_gmt() {
        let dt = parse_http_date("Fri, 02 Jan 2026 03:04:05 GMT").unwrap();
        assert_eq!(dt.to_rfc3339(), "2026-01-02T03:04:05+00:00");
        assert!(parse_http_date("junk").is_none());
    }

    #[test]
    fn parse_list_blobs_handles_empty_marker_and_entities() {
        let xml = list_xml(
            "<Blob><Name>a b.json</Name><Properties>\
             <Creation-Time>Thu, 01 Jan 2026 00:00:01 GMT</Creation-Time>\
             </Properties></Blob>",
            "",
        );
        let (entries, next) = parse_list_blobs(&xml);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "a b.json");
        assert!(next.unwrap_or_default().is_empty());
    }

    #[test]
    fn constructor_validation_matches_python_messages() {
        let err = AzureBlobBackend::new("", "c").unwrap_err();
        assert!(
            err.to_string()
                .contains("WC_AZURE_STORAGE_ACCOUNT env var is empty"),
            "{err}"
        );
        let err = AzureBlobBackend::new("a", "").unwrap_err();
        assert!(
            err.to_string()
                .contains("WC_AZURE_CONTAINER env var is empty"),
            "{err}"
        );
        let ok = AzureBlobBackend::new("a", "c").unwrap();
        assert_eq!(ok.account(), "a");
        assert_eq!(ok.container(), "c");
    }
}
