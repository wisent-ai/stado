//! The version-carrying read and the conditional write it pairs with.
//!
//! `objects/mod.rs` carries the trait entries; both routes are here because
//! they share the `x-stado-version` contract and nothing else does.

use reqwest::{Method, StatusCode};
use serde::Deserialize;

use crate::queue::{StorageError, VersionedText};

use super::super::{StadoObjectBackend, VERSION_HEADER};

impl StadoObjectBackend {
    pub(super) async fn download_versioned_text(
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
        let header = response.headers().get(VERSION_HEADER).ok_or_else(|| {
            StorageError::Other(format!("Stado object API omitted {VERSION_HEADER} for {path}"))
        })?;
        let version = header
            .to_str()
            .map_err(|error| {
                StorageError::Other(format!(
                    "Stado object API sent a {VERSION_HEADER} for {path} that is not visible ASCII: {error}"
                ))
            })?
            .to_string();
        if version.is_empty() {
            return Err(StorageError::Other(format!(
                "Stado object API sent an empty {VERSION_HEADER} for {path}"
            )));
        }
        // The same measured room as the unversioned read: this is the route
        // the canonical registry and every conditional-write document take.
        let room = super::text_room(path);
        let bytes = Self::whole_body(response, path, room).await?;
        let content = String::from_utf8(bytes)
            .map_err(|error| StorageError::Other(format!("invalid UTF-8 in {path}: {error}")))?;
        Ok(Some(VersionedText { content, version }))
    }

    pub(super) async fn swap_text_if_version(
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
}
