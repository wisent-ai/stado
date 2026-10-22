//! The paginated `objects.list` walks: the whole-prefix scan behind sorted
//! names, the bounded server-side page, and the full descriptor set.
//!
//! `objects/mod.rs` carries the trait entries; the walks are here because
//! `nextPageToken` is the whole of them — a caller either drains the prefix
//! or stops the moment its window is full — and they differ only in the
//! `fields` projection each one asks for. An item the service lists without
//! a field the walk asked for is refused with the item, never read as empty.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use reqwest::Method;
use serde_json::Value;

use crate::queue::{BlobInfo, Listing, StorageError};

use super::super::{
    client::parse_timestamp,
    refusals::ensure_success,
    uri::{list_page_url, list_url},
    GcsBackend,
};

/// The string field `field` of one listed item, refused when the service
/// left it out or wrote it as anything but a string.
fn text<'a>(item: &'a Value, field: &str) -> Result<&'a str, StorageError> {
    item.get(field).and_then(Value::as_str).ok_or_else(|| {
        StorageError::Other(format!(
            "GCS objects.list answered an item without a string {field}: {item}"
        ))
    })
}

/// The items of one listed page; a page with none omits the field.
fn items(page: &Value) -> &[Value] {
    match page.get("items").and_then(Value::as_array) {
        Some(items) => items,
        None => &[],
    }
}

/// The custom metadata of one listed item: the service omits the field when
/// the object carries none, and every value it writes is a string.
fn metadata(item: &Value) -> Result<BTreeMap<String, String>, StorageError> {
    let Some(fields) = item.get("metadata") else {
        return Ok(BTreeMap::new());
    };
    let fields = fields.as_object().ok_or_else(|| {
        StorageError::Other(format!(
            "GCS objects.list answered metadata that is not an object: {item}"
        ))
    })?;
    fields
        .iter()
        .map(|(key, value)| match value.as_str() {
            Some(value) => Ok((key.clone(), value.to_string())),
            None => Err(StorageError::Other(format!(
                "GCS objects.list answered metadata {key} that is not a string: {item}"
            ))),
        })
        .collect()
}

/// The byte size of one listed item, which the service writes as a decimal
/// string; absent when the projection did not carry it.
fn size(item: &Value) -> Result<Option<u64>, StorageError> {
    match item.get("size").and_then(Value::as_str) {
        None => Ok(None),
        Some(written) => written.parse::<u64>().map(Some).map_err(|error| {
            StorageError::Other(format!(
                "GCS objects.list answered a size {written:?} that is no byte count: {error}"
            ))
        }),
    }
}

impl GcsBackend {
    pub(super) async fn list_sorted_paths(
        &self,
        prefix: &str,
        listing: Listing,
    ) -> Result<Vec<String>, StorageError> {
        let fields = "items(name,timeCreated),nextPageToken";
        let mut listed: Vec<(String, String)> = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let url = list_url(&self.inner.bucket, prefix, page_token.as_deref(), fields);
            let response = self.send(Method::GET, &url, None).await?;
            let page: Value = ensure_success(response).await?.json().await?;
            for item in items(&page) {
                let name = text(item, "name")?;
                let created = text(item, "timeCreated")?;
                listed.push((name.to_string(), created.to_string()));
            }
            match page.get("nextPageToken").and_then(Value::as_str) {
                Some(token) => page_token = Some(token.to_string()),
                None => break,
            }
        }
        if let Listing::Oldest(count) = listing {
            listed.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
            listed.truncate(count.get());
        }
        Ok(listed.into_iter().map(|(name, _)| name).collect())
    }

    /// objects.list already answers in lexicographic name order, so
    /// `startOffset` and `maxResults` express this page server-side. The
    /// generic default would have followed every `nextPageToken` of the
    /// prefix — 14k+ names off the `queue/` index, at 1000 per round-trip —
    /// and sorted them locally only to keep the first few. The one asymmetry
    /// is that `startOffset` is inclusive while `start_after` is exclusive:
    /// the boundary name is discarded here, and since that discard can leave
    /// a `maxResults`-sized page one name short, paging continues until the
    /// limit is filled or the prefix runs out.
    pub(super) async fn list_name_page(
        &self,
        prefix: &str,
        start_after: &str,
        limit: Option<NonZeroUsize>,
    ) -> Result<Vec<String>, StorageError> {
        let fields = "items(name),nextPageToken";
        let mut out: Vec<String> = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            // Only the shortfall, so a resumed walk never over-fetches.
            let max_results = limit.map(|limit| limit.get() - out.len());
            let url = list_page_url(
                &self.inner.bucket,
                prefix,
                page_token.as_deref(),
                fields,
                start_after,
                max_results,
            );
            let response = self.send(Method::GET, &url, None).await?;
            let page: Value = ensure_success(response).await?.json().await?;
            for item in items(&page) {
                let name = text(item, "name")?;
                if name == start_after {
                    continue;
                }
                out.push(name.to_string());
                if limit.is_some_and(|limit| out.len() >= limit.get()) {
                    return Ok(out);
                }
            }
            match page.get("nextPageToken").and_then(Value::as_str) {
                Some(token) => page_token = Some(token.to_string()),
                None => break,
            }
        }
        Ok(out)
    }

    pub(super) async fn list_blobs(&self, prefix: &str) -> Result<Vec<BlobInfo>, StorageError> {
        let fields = "items(name,updated,size,metadata),nextPageToken";
        let mut out = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let url = list_url(&self.inner.bucket, prefix, page_token.as_deref(), fields);
            let response = self.send(Method::GET, &url, None).await?;
            let page: Value = ensure_success(response).await?.json().await?;
            for item in items(&page) {
                out.push(BlobInfo {
                    name: text(item, "name")?.to_string(),
                    updated: parse_timestamp(item.get("updated"), text(item, "name")?)?,
                    size: size(item)?,
                    metadata: metadata(item)?,
                });
            }
            match page.get("nextPageToken").and_then(Value::as_str) {
                Some(token) => page_token = Some(token.to_string()),
                None => break,
            }
        }
        Ok(out)
    }
}
