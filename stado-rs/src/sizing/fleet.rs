//! The live-capacity ladder: the REAL GPU sizes the fleet is broadcasting.

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::queue::{JobStorage, StorageError};

use super::{download_many, Sizing};

impl Sizing {
    /// Smallest REAL GPU total_vram_gb currently in the fleet, or None if
    /// no live agent is broadcasting (then the caller must not invent a
    /// number — the job stays unsized until a real GPU appears).
    /// Python `smallest_live_vram`.
    pub async fn smallest_live_vram(
        &self,
        store: &JobStorage,
    ) -> Result<Option<i64>, StorageError> {
        Ok(self.live_total_vrams(store).await?.first().copied())
    }

    /// Smallest REAL fleet total_vram_gb strictly greater than `current`,
    /// or None if no live GPU is bigger (genuine ceiling — not a guess).
    /// Python `next_live_vram`.
    pub async fn next_live_vram(
        &self,
        store: &JobStorage,
        current: i64,
    ) -> Result<Option<i64>, StorageError> {
        Ok(self
            .live_total_vrams(store)
            .await?
            .into_iter()
            .find(|v| *v > current))
    }

    /// Ascending, de-duplicated list of the REAL total_vram_gb values the
    /// fleet is broadcasting right now — i.e. the actual GPUs that exist,
    /// read from <bucket>/capacity/ (each agent publishes its own
    /// nvidia-smi total_vram_gb). No catalog, no hand-written tier list.
    /// A broadcast past the next one its publisher promised is excluded
    /// ([`crate::queue::capacity::publication_live`]). Read on every call:
    /// a copy held for a chosen number of seconds answered with GPUs whose
    /// agents had already stopped promising anything.
    /// Python `_live_total_vrams`.
    pub async fn live_total_vrams(&self, store: &JobStorage) -> Result<Vec<i64>, StorageError> {
        let now = Utc::now();
        let mut vrams: Vec<i64> = Vec::new();
        let paths = store.list_paths("capacity/", 0).await?;
        for text in download_many(store, &paths).await?.into_iter().flatten() {
            let doc: Value = serde_json::from_str(&text)?;
            let Some(pub_at) = doc.get("published_at").and_then(Value::as_str) else {
                continue;
            };
            // Python `except Exception: continue` — an unparseable
            // published_at just drops the broadcast.
            let Ok(published) = DateTime::parse_from_rfc3339(pub_at) else {
                continue;
            };
            let stamp = Some(published.with_timezone(&Utc));
            if !crate::queue::capacity::publication_live(&doc, stamp, now) {
                continue;
            }
            if let Some(tv) = doc.get("total_vram_gb").and_then(Value::as_i64) {
                if tv > 0 && !vrams.contains(&tv) {
                    vrams.push(tv);
                }
            }
        }
        vrams.sort_unstable();
        Ok(vrams)
    }
}
