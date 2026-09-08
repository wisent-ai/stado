//! The object reads: the resource GET, the media GET, and the
//! generation-pinned read built on the pair of them.
//!
//! `objects/mod.rs` carries the trait entries; the routes are here because
//! they share the generation the versioned read pins, and the retry it owes
//! a writer that bumps the object between the two round-trips.

use reqwest::{Method, StatusCode};

use crate::queue::{StorageError, VersionedText};

use super::super::{
    refusals::ensure_success,
    uri::{media_url, object_url},
    GcsBackend,
};

impl GcsBackend {
    /// GET the object resource (generation / updated / metadata).
    pub(super) async fn get_object(
        &self,
        path: &str,
    ) -> Result<Option<serde_json::Value>, StorageError> {
        let url = object_url(&self.inner.bucket, path);
        let response = self.send(Method::GET, &url, None).await?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(ensure_success(response).await?.json().await?))
    }

    /// GET the object media, or `None` on 404. `if_generation_match` pins
    /// the read to a specific generation (versioned reads).
    pub(super) async fn get_media(
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

    pub(super) async fn download_versioned_text(
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
}
