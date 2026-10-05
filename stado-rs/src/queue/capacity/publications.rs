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

    /// Past the promise its author made ([`next_publication_by`]), or making
    /// none. An undateable row from an older publisher cannot be held to its
    /// window and is not live either: nothing says when it meant to speak
    /// again.
    pub fn stale(&self, now: DateTime<Utc>) -> bool {
        !publication_live(&self.payload, self.stamp, now)
    }

    /// When this row's author promised its next publication, if it said.
    pub fn next_by(&self) -> Option<DateTime<Utc>> {
        next_publication_by(&self.payload, self.stamp)
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
        let stamp = published_stamp(&payload, blob.updated);
        rows.insert(consumer_id, Publication { payload, stamp });
    }
    Ok(rows)
}

/// Return {consumer_id: payload} for every live consumer: one still within
/// the promise of its own publication ([`publication_live`]).
/// Python `read_consumer_capacity`.
pub async fn read_consumer_capacity(
    store: &JobStorage,
) -> Result<BTreeMap<String, Value>, StorageError> {
    read_consumer_capacity_at(store, Utc::now()).await
}

/// [`read_consumer_capacity`] with an injectable clock.
///
/// Every row is read, because its liveness is in its body (`next_by`), not
/// in the object's age. What keeps that read small is collection: a row of a
/// cloud consumer whose promise has passed is deleted where it is found,
/// because its VM is gone or no longer speaking and the row is evidence about
/// nothing the fleet still holds. A `local` host keeps its last row however
/// old: there is one per declared machine, and it is the evidence that the
/// machine went quiet. Nothing here is capped per tick: a tick deletes what
/// is due.
async fn read_consumer_capacity_at(
    store: &JobStorage,
    now: DateTime<Utc>,
) -> Result<BTreeMap<String, Value>, StorageError> {
    let mut out: BTreeMap<String, Value> = BTreeMap::new();
    for blob in store.list_blobs_with_meta(CAPACITY_PREFIX).await? {
        if !blob.name.ends_with(".json") {
            continue;
        }
        // Race: an agent can self-delete its own broadcast (or another tick
        // can sweep it) between list above and download below. Both
        // backends translate the 404 into a None return so the missing-blob
        // case is the only one dropped; any other error propagates so
        // transient SDK/network failures stay visible.
        let Some(raw) = store.download_text(&blob.name).await? else {
            continue;
        };
        let payload: Value = serde_json::from_str(&raw)?;
        let stamp = published_stamp(&payload, blob.updated);
        if publication_live(&payload, stamp, now) {
            if let Some(cid) = payload.get("consumer_id").and_then(Value::as_str) {
                out.insert(cid.to_string(), payload);
            }
            continue;
        }
        if payload.get("kind").and_then(Value::as_str) != Some("local") {
            store.delete_blob(&blob.name).await?;
        }
    }
    Ok(out)
}
