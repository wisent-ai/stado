//! The paginated List Blobs walk, and the entry one page decodes into.
//!
//! `objects/mod.rs` carries the trait entries; the walk is here because the
//! opaque continuation marker is the whole of it: a caller either drains the
//! prefix or stops the moment its window is full, and both are the same loop.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use reqwest::Method;

use crate::queue::StorageError;

use super::super::AzureBlobBackend;
use super::xml::parse_list_blobs;

impl AzureBlobBackend {
    /// Drive the paginated List Blobs walk, handing each parsed page to
    /// `consume` as it arrives. The walk ends when the continuation marker
    /// is exhausted or `consume` returns `false`, so a caller that only
    /// wants a window can stop paging instead of draining the prefix.
    pub(super) async fn walk_list_pages(
        &self,
        prefix: &str,
        include_metadata: bool,
        max_results: Option<usize>,
        mut consume: impl FnMut(Vec<ListEntry>) -> bool,
    ) -> Result<(), StorageError> {
        let mut marker: Option<String> = None;
        loop {
            let url = self.list_url(prefix, marker.as_deref(), include_metadata, max_results);
            let response = self.send(Method::GET, &url, &[], None).await?;
            let body = Self::ensure_success(response, &format!("list {prefix}"))
                .await?
                .text()
                .await?;
            let (entries, next) = parse_list_blobs(&body);
            if !consume(entries) {
                break;
            }
            match next {
                Some(next) if !next.is_empty() => marker = Some(next),
                _ => break,
            }
        }
        Ok(())
    }

    /// One paginated List Blobs walk, parsed into raw entries.
    pub(super) async fn list_entries(
        &self,
        prefix: &str,
        include_metadata: bool,
    ) -> Result<Vec<ListEntry>, StorageError> {
        let mut out = Vec::new();
        self.walk_list_pages(prefix, include_metadata, None, |entries| {
            out.extend(entries);
            true
        })
        .await?;
        Ok(out)
    }
}

/// One `<Blob>` entry of a List Blobs response.
pub(super) struct ListEntry {
    pub(super) name: String,
    pub(super) creation_time: Option<DateTime<Utc>>,
    pub(super) last_modified: Option<DateTime<Utc>>,
    pub(super) size: Option<u64>,
    pub(super) metadata: BTreeMap<String, String>,
}
