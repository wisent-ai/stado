//! The URLs every request is addressed through.
//!
//! One `/{container}/{blob}` URL per blob path and one List Blobs URL per
//! page, so the per-segment percent-encoding and the pagination query are
//! written once instead of at each route.

use crate::queue::gcs::percent_encode;

use super::AzureBlobBackend;

impl AzureBlobBackend {
    /// `/{container}/{path}` with the blob name percent-encoded per segment
    /// (slash separators preserved).
    pub(super) fn blob_url(&self, path: &str) -> String {
        let encoded = path
            .split('/')
            .map(percent_encode)
            .collect::<Vec<_>>()
            .join("/");
        format!("{}/{}/{encoded}", self.inner.base_url, self.inner.container)
    }

    /// Container List Blobs URL; `marker` continues a paginated listing,
    /// `include_metadata` adds each blob's metadata to the response
    /// (Python `include=["metadata"]`), `max_results` caps one page
    /// (`maxresults`; absent = the service default of 5000).
    pub(super) fn list_url(
        &self,
        prefix: &str,
        marker: Option<&str>,
        include_metadata: bool,
        max_results: Option<usize>,
    ) -> String {
        let mut url = format!(
            "{}/{}?restype=container&comp=list&prefix={}",
            self.inner.base_url,
            self.inner.container,
            percent_encode(prefix)
        );
        if include_metadata {
            url.push_str("&include=metadata");
        }
        if let Some(max_results) = max_results {
            url.push_str(&format!("&maxresults={max_results}"));
        }
        if let Some(marker) = marker {
            url.push_str(&format!("&marker={}", percent_encode(marker)));
        }
        url
    }
}
