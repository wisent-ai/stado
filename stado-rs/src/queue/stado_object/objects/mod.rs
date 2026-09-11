//! The [`BlobBackend`] surface: every queue storage operation the rest of the
//! crate reaches this backend through.
//!
//! The bodies that are a seam of their own live beside this file — the
//! buffered reads and writes ([`transfer`]), the stat read and its timestamp
//! parse ([`metadata`]), the whole-prefix listings ([`listing`]) and the
//! `x-stado-version` pair ([`versioned`]).

use std::collections::BTreeMap;
use std::path::Path;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reqwest::{Method, StatusCode};

use crate::queue::{BlobBackend, BlobInfo, StorageError, VersionedText};
use crate::remote::object_store::ObjectRef;

use super::StadoObjectBackend;

mod listing;
mod metadata;
mod transfer;
mod versioned;

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
        // The trailing separator travels with the request: `queue/` means the
        // queue's own objects, and without it the gateway answers with every
        // sibling whose name starts with `queue` — `queue_priority/`'s 9026
        // markers among them, which `list_jobs` then tries to parse as jobs.
        Ok(prefix.trim_start_matches('/').to_string())
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
        // Every text object this store serves is a document: a registry, a
        // policy, a job, a capacity row, a queue-control record. Their sizes
        // are part of their contract, so they read under the document
        // ceiling and a reply that declares more than that never lands in
        // this process at all.
        let Some(bytes) = self
            .download_bytes_limited(
                path,
                Some(crate::primitives::constants::STORE_DOCUMENT_MAX_BYTES),
            )
            .await?
        else {
            return Ok(None);
        };
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|error| StorageError::Other(format!("invalid UTF-8 in {path}: {error}")))
    }

    async fn download_bytes(&self, path: &str) -> Result<Option<Vec<u8>>, StorageError> {
        // Unlimited: this is also the route a software artifact takes on its
        // way to `download_to_filename`.
        self.download_bytes_limited(path, None).await
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
    ///
    /// The body goes through [`Self::whole_body`] for the same reason the
    /// object route's does. This route answers with a `Content-Length` too,
    /// and this reader used to return `response.bytes()` without ever
    /// comparing the two — so an answer that ended cleanly at a size its own
    /// declaration contradicted became the archive. A short document is
    /// journalled as a document that does not parse; a short archive is
    /// worse, because it unpacks, and the declared length was the only thing
    /// on the wire that could have said it was not the whole thing.
    ///
    /// No ceiling: a software artifact is legitimately large and is on its
    /// way to a file.
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
        Ok(Some(Self::whole_body(response, uri, None).await?))
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
        self.download_versioned_text(path).await
    }

    async fn compare_and_swap_text(
        &self,
        path: &str,
        expected_version: &str,
        content: &str,
    ) -> Result<String, StorageError> {
        self.swap_text_if_version(path, expected_version, content)
            .await
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
        self.list_sorted_paths(prefix, oldest_first).await
    }

    async fn list_page(
        &self,
        prefix: &str,
        start_after: &str,
        limit: usize,
    ) -> Result<Vec<String>, StorageError> {
        self.list_key_page(prefix, start_after, limit).await
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
        self.list_blobs(prefix).await
    }
}
