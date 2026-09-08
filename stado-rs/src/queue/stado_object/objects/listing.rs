//! The three listing reads behind the [`BlobBackend`] surface: sorted names,
//! one client-side page of keys, and the full descriptor set.
//!
//! `objects/mod.rs` carries the trait entries; the work is here because the
//! whole-prefix response and its metadata parse are one seam.

use reqwest::Method;

use crate::queue::{BlobBackend, BlobInfo, StorageError};

use super::super::{ObjectList, StadoObjectBackend};

impl StadoObjectBackend {
    pub(super) async fn list_sorted_paths(
        &self,
        prefix: &str,
        oldest_first: usize,
    ) -> Result<Vec<String>, StorageError> {
        let mut blobs = self.list_blobs_with_meta(prefix).await?;
        if oldest_first > 0 {
            blobs.sort_by(|left, right| {
                left.updated
                    .cmp(&right.updated)
                    .then(left.name.cmp(&right.name))
            });
            blobs.truncate(oldest_first);
        } else {
            blobs.sort_by(|left, right| left.name.cmp(&right.name));
        }
        Ok(blobs.into_iter().map(|blob| blob.name).collect())
    }

    /// The gateway has no server-side cursor to use: `/api/object/list` takes
    /// a namespace and a prefix, and answers with the whole authorized prefix
    /// as descriptors — there is no offset, no limit, and no name-only
    /// projection to ask for, and inventing one would page against a server
    /// that ignores it and silently return the wrong window. So the cut stays
    /// on this side and the round-trip is the same one [`Self::list_paths`]
    /// would make.
    ///
    /// What the override does buy is the metadata work behind that response.
    /// The default reaches `list_paths`, which builds a [`BlobInfo`] per
    /// object and parses every RFC 3339 `updated_at` in the prefix — 14k
    /// timestamp parses and 14k metadata maps to answer a request for a few
    /// names that need none of it. Worse, that parse is fallible, so one
    /// malformed timestamp anywhere under the prefix fails a page that would
    /// never have returned the object. Reading only the keys is both cheaper
    /// and harder to break, and this is the single place to add a cursor when
    /// the gateway grows one.
    pub(super) async fn list_key_page(
        &self,
        prefix: &str,
        start_after: &str,
        limit: usize,
    ) -> Result<Vec<String>, StorageError> {
        let response =
            Self::send_through_boundary(self.request(Method::GET, self.list_url(prefix)?)).await?;
        if !response.status().is_success() {
            return Err(Self::response_error(response).await);
        }
        let payload: ObjectList = response.json().await?;
        let mut names: Vec<String> = payload
            .objects
            .into_iter()
            .map(|object| object.key)
            .collect();
        names.sort_unstable();
        let cut = names.partition_point(|name| name.as_str() <= start_after);
        names.drain(..cut);
        if limit > 0 {
            names.truncate(limit);
        }
        Ok(names)
    }

    pub(super) async fn list_blobs(&self, prefix: &str) -> Result<Vec<BlobInfo>, StorageError> {
        let url = self.list_url(prefix)?;
        let response = Self::send_through_boundary(self.request(Method::GET, url)).await?;
        if !response.status().is_success() {
            return Err(Self::response_error(response).await);
        }
        let payload: ObjectList = response.json().await?;
        // Keep only what was asked for. A gateway that drops the trailing
        // separator answers a request for `queue/` with every `queue*`
        // sibling, and a client that trusts the filter reads 9026
        // `queue_priority/` markers as queued jobs. Filtering here means a
        // fleet still running that gateway cannot make this reader wrong.
        let requested = self.blob_prefix(&self.namespace, prefix)?;
        payload
            .objects
            .into_iter()
            .filter(|object| object.key.starts_with(&requested))
            .map(|object| {
                Ok(BlobInfo {
                    name: object.key,
                    updated: Self::parse_updated(object.updated_at)?,
                    size: object.size,
                    metadata: object.metadata,
                })
            })
            .collect()
    }
}
