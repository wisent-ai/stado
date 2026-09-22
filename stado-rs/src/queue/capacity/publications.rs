//! What the fleet's consumers have published about their own capacity, and
//! the two readers of it.
//!
//! Split out of `capacity/mod.rs`, which had grown past the module line cap;
//! the horizons and the snapshot stay there.

use super::*;

/// One consumer's publication and the instant it says it was made.
///
/// Carries the stamp separately from the payload because every consumer of a
/// publication asks the same second question — how old is this — and reading
/// `published_at` out of the body at each site is how two surfaces end up
/// disagreeing about whether a row is stale.
#[derive(Debug, Clone, PartialEq)]
pub struct Publication {
    /// The row exactly as its author wrote it.
    pub payload: Value,
    /// `published_at` from the body, falling back to the object's own
    /// timestamp for a body that predates the field or carries an
    /// unparseable one, so a row can never be reported as ageless.
    pub stamp: Option<DateTime<Utc>>,
}

impl Publication {
    /// Seconds since this row was published, or `None` when neither the body
    /// nor the object could say when that was.
    pub fn age_seconds(&self, now: DateTime<Utc>) -> Option<i64> {
        self.stamp.map(|stamp| (now - stamp).num_seconds())
    }

    /// Past [`CAPACITY_STALE_SECONDS`], the horizon every live-capacity
    /// reader in the fleet filters on. An undateable row is NOT stale: it is
    /// a row of unknown age, and calling it stale would invent a fact.
    pub fn stale(&self, now: DateTime<Utc>) -> bool {
        self.age_seconds(now)
            .is_some_and(|age| age > CAPACITY_STALE_SECONDS as i64)
    }
}

/// Return {consumer_id: publication} for EVERY row under
/// [`CAPACITY_PREFIX`], stale ones included, deleting nothing.
///
/// The reader for reports, never for a tick. [`read_consumer_capacity`] is
/// the scheduler's reader and is wrong for anything that has to explain a
/// silent fleet twice over: it drops every row past the staleness horizon,
/// and it DELETES every row past the GC horizon — so an operator command
/// built on it destroys the evidence that a host went quiet an hour ago and
/// then reports that the host never said anything at all.
///
/// The cost that justified the GC-ing reader's metadata prefilter does not
/// apply here: that was a 60s Cloud Function tick against 1900+ accumulated
/// rows, and the tick still runs that reader and still collects them. A
/// report runs once, on an operator's keystroke, against whatever the tick
/// has left.
pub async fn read_publications(
    store: &JobStorage,
) -> Result<BTreeMap<String, Publication>, StorageError> {
    let mut rows: BTreeMap<String, Publication> = BTreeMap::new();
    for blob in store.list_blobs_with_meta(CAPACITY_PREFIX).await? {
        let Some(stem) = blob
            .name
            .strip_prefix(CAPACITY_PREFIX)
            .and_then(|name| name.strip_suffix(".json"))
        else {
            continue;
        };
        // Race: an agent can self-delete its own broadcast, or a scheduler
        // tick can sweep it, between the listing above and the download
        // below. A missing blob is the one case dropped; every other error
        // propagates so a broken store never reads as a quiet fleet.
        let Some(raw) = store.download_text(&blob.name).await? else {
            continue;
        };
        let payload: Value = serde_json::from_str(&raw)?;
        let consumer_id = payload
            .get("consumer_id")
            .and_then(Value::as_str)
            .unwrap_or(stem)
            .to_string();
        let stamp = payload
            .get("published_at")
            .and_then(Value::as_str)
            .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
            .map(|stamp| stamp.with_timezone(&Utc))
            .or(blob.updated);
        rows.insert(consumer_id, Publication { payload, stamp });
    }
    Ok(rows)
}

/// Return {consumer_id: payload} for every live (non-stale) consumer.
/// Python `read_consumer_capacity`.
pub async fn read_consumer_capacity(
    store: &JobStorage,
) -> Result<BTreeMap<String, Value>, StorageError> {
    read_consumer_capacity_at(store, Utc::now()).await
}

/// [`read_consumer_capacity`] with an injectable clock so the staleness and
/// GC windows are testable without backdating blob mtimes (Python reads the
/// clock inline).
///
/// Filters on blob.updated metadata BEFORE downloading. Previously every
/// tick downloaded all broadcast files (1900+ accumulated, most stale)
/// just to read published_at — at ~30ms/blob this outran the 60s the Cloud
/// Function is given, returned 504, and Cloud Scheduler auto-paused the
/// cron. Filtering on server-side metadata first means the tick reads
/// only the small number of fresh blobs.
///
/// Also deletes long-stale blobs (older than 1h, well past
/// CAPACITY_STALE_SECONDS=180s) so the bucket can't accumulate forever.
/// Capped per tick so the Cloud Function never spends its budget on GC.
async fn read_consumer_capacity_at(
    store: &JobStorage,
    now: DateTime<Utc>,
) -> Result<BTreeMap<String, Value>, StorageError> {
    let cutoff_fresh = now - Duration::seconds(CAPACITY_STALE_SECONDS as i64);
    let cutoff_delete = now - Duration::seconds(CAPACITY_GC_AGE_SECONDS);

    let mut out: BTreeMap<String, Value> = BTreeMap::new();
    let mut stale_blobs: Vec<String> = Vec::new();
    for blob in store.list_blobs_with_meta(CAPACITY_PREFIX).await? {
        if !blob.name.ends_with(".json") {
            continue;
        }
        let Some(updated) = blob.updated else {
            continue;
        };
        if updated < cutoff_delete {
            stale_blobs.push(blob.name);
            continue;
        }
        if updated < cutoff_fresh {
            continue;
        }
        // Race: an agent can self-delete its own broadcast (or another tick
        // can sweep stale broadcasts) between list above and download below.
        // Both backends translate the 404 into a None return so the
        // missing-blob case is the only one we drop; any other error
        // propagates to the caller so transient SDK/network failures stay
        // visible.
        let Some(raw) = store.download_text(&blob.name).await? else {
            continue;
        };
        let payload: Value = serde_json::from_str(&raw)?;
        if let Some(cid) = payload.get("consumer_id").and_then(Value::as_str) {
            out.insert(cid.to_string(), payload);
        }
    }

    for name in stale_blobs.into_iter().take(CAPACITY_GC_CAP_PER_TICK) {
        store.delete_blob(&name).await?;
    }
    Ok(out)
}
