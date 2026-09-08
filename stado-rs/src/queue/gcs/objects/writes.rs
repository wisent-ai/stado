//! The object writes: the media upload every write goes out through, the
//! compare-and-swap it becomes under a generation precondition, and the
//! metadata patch.
//!
//! `objects/mod.rs` carries the trait entries; the routes are here because
//! `ifGenerationMatch` is the whole of them — absent for a plain write, "0"
//! for a create, an observed generation for a swap — and the metadata patch
//! is the one write that has to read the object first.

use std::collections::BTreeMap;

use reqwest::{Method, StatusCode};

use crate::queue::StorageError;

use super::super::{
    refusals::ensure_success,
    uri::{object_url, upload_url},
    GcsBackend,
};

impl GcsBackend {
    /// Upload `content`, optionally guarded by an `ifGenerationMatch`
    /// precondition ("0" = create-only, generation = CAS).
    pub(super) async fn upload(
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

    pub(super) async fn swap_text_if_generation(
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

    pub(super) async fn patch_metadata(
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
}
