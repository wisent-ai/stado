//! The paginated `objects.list` walks: the whole-prefix scan behind sorted
//! names, the bounded server-side page, and the full descriptor set.
//!
//! `objects/mod.rs` carries the trait entries; the walks are here because
//! `nextPageToken` is the whole of them — a caller either drains the prefix
//! or stops the moment its window is full — and they differ only in the
//! `fields` projection each one asks for.

use std::collections::BTreeMap;

use reqwest::Method;

use crate::queue::{BlobInfo, StorageError};

use super::super::{
    client::parse_timestamp,
    refusals::ensure_success,
    uri::{list_page_url, list_url},
    GcsBackend,
};

impl GcsBackend {
    pub(super) async fn list_sorted_paths(
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
        limit: usize,
    ) -> Result<Vec<String>, StorageError> {
        let fields = "items(name),nextPageToken";
        let mut out: Vec<String> = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            // Only the shortfall, so a resumed walk never over-fetches.
            let max_results = if limit > 0 { limit - out.len() } else { 0 };
            let url = list_page_url(
                &self.inner.bucket,
                prefix,
                page_token.as_deref(),
                fields,
                start_after,
                max_results,
            );
            let response = self.send(Method::GET, &url, None).await?;
            let page: serde_json::Value = ensure_success(response).await?.json().await?;
            if let Some(array) = page.get("items").and_then(|i| i.as_array()) {
                for item in array {
                    let name = item
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or_default();
                    if name == start_after {
                        continue;
                    }
                    out.push(name.to_string());
                    if limit > 0 && out.len() >= limit {
                        return Ok(out);
                    }
                }
            }
            match page.get("nextPageToken").and_then(|t| t.as_str()) {
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
